//! Home: the widget grid under the composer on the new-chat screen
//! (docs/plan/04-home.md in the Keron repo).
//!
//! The runtime is the keron-home crate: manifests in `~/.keron/widgets`, the
//! layout in `~/.keron/home.toml`, each kind's JSON, and fetching. This
//! module is the view on top of it. It loads and watches the widgets folder,
//! runs one refresh loop per shown widget, signs in to the door, draws each
//! kind ([`view`]) and owns Customize. The shell hands it the app's own data
//! as a snapshot ([`snapshot`]) and where to sit ([`Home::set_frame`]), and
//! listens for [`HomeEvent`]s.

mod bridge;
mod view;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt as _;
use gpui::{
    AppContext as _, Bounds, Context, EventEmitter, Pixels, Point, ScrollHandle, Task, point,
};
use gpui_tokio::Tokio;
use keron_door::DoorClient;
use keron_home::catalog::{self, Catalog};
use keron_home::kinds::{is_openable, parse_chat_link};
use keron_home::loose_ends::{self, Action};
use keron_home::script::ScriptOptions;
use keron_home::{
    Body, FetchError, Fetcher, HomePaths, Layout, ListItem, Manifest, Payload, SourceSpec,
    ZeronSnapshot,
};

use crate::popover::Popup;

pub(crate) use bridge::snapshot;

/// The draft "Describe a widget…" starts a chat with.
pub(crate) const DESCRIBE_WIDGET_STARTER: &str = "Make me a Keron Home widget that shows: \n\nPut its manifest in ~/.keron/widgets/<id>.toml and, if it needs one, a script next to it that prints the widget's JSON. ~/.keron/widgets/README.md has the format. Home picks it up by itself, no rebuild needed.";

/// Layout changes settle this long before home.toml is written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// `zeron:` payloads carry ages, so they're recomputed this often.
const ZERON_TICK: Duration = Duration::from_secs(10);
/// Loose ends on screen are reported at most this often.
const SHOWN_EVERY: Duration = Duration::from_secs(60 * 60);
/// Rows a card shows when its manifest has no `limit`.
const DEFAULT_LIMIT: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub enum HomeEvent {
    /// Start a new chat with this draft, asking an agent for a widget.
    DescribeWidget(String),
    /// Open one of the app's own chats.
    OpenChat(String),
    /// Which widgets show changed; the shell keeps pull requests watched
    /// while that widget shows.
    LayoutChanged,
}

/// Where Home sits, from the shell: the top of the grid in window
/// coordinates (just under the composer), the composer column's width, and
/// the route fade.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Frame {
    top: f32,
    width: f32,
    opacity: f32,
}

/// The app's sign-in to the door, which every `keron-sources:` and
/// `memory:` widget needs.
#[derive(Clone, Debug, PartialEq)]
enum SignIn {
    /// Reading the Keychain.
    Checking,
    /// No door client (tests).
    NoDoor,
    SignedOut,
    /// Discovery and registration, before the browser opens.
    Opening,
    /// The browser is open on the sign-in page.
    Waiting,
    SignedIn,
    Failed(String),
}

/// What Home knows about one widget. Dropping it stops its refresh loop and
/// any action in flight.
#[derive(Default)]
struct WidgetState {
    payload: Option<Payload>,
    error: Option<String>,
    /// Why a `zeron:` source has nothing to give yet.
    unavailable: Option<String>,
    loading: bool,
    refresh: Option<Task<()>>,
    /// Optimistic loose-ends changes, by row id.
    overrides: HashMap<String, RowOverride>,
    actions: HashMap<String, Task<()>>,
    /// Why the last action failed, and when. A good fetch that began after
    /// it clears it.
    action_error: Option<(String, Instant)>,
    shown_posted: Option<Instant>,
    shown_task: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RowChange {
    /// Done or "not a thing": the row leaves.
    Hidden,
    Snoozed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RowOverride {
    change: RowChange,
    /// The door took it; the next payload replaces it.
    settled: bool,
}

/// One widget in Home's order.
#[derive(Clone, Debug)]
struct Slot {
    manifest: Manifest,
    shown: bool,
    width: u8,
}

/// How a widget gets its data right now.
enum Mode {
    /// Recomputed from the shell's snapshot.
    Zeron,
    Fetch,
    /// Signed out of the door, or scripts still waiting for the PATH.
    Wait,
}

/// A card being dragged in Customize (gpui drag and drop).
pub struct CardDragPayload {
    id: String,
    /// Its index among the shown cards when the drag began.
    from: usize,
}

/// Where the dragged card would land. `slots` are the shown cards' bounds
/// when the drag began, by index: a card dropped over slot `n` lands at `n`.
struct CardDrag {
    id: String,
    from: usize,
    over: usize,
    prev_over: usize,
    epoch: usize,
    slots: Vec<Bounds<Pixels>>,
    scroll_y: Pixels,
}

pub struct Home {
    paths: HomePaths,
    fetcher: Fetcher,
    /// Script sources wait for the login shell's PATH.
    scripts_ready: bool,
    /// Watch the widgets folder once the first load is in.
    watch_folder: bool,
    loaded: bool,
    catalog: Catalog,
    layout: Layout,
    /// Why home.toml couldn't be read. While set, changes aren't saved, so
    /// the owner's file isn't overwritten.
    layout_error: Option<String>,
    widgets: HashMap<String, WidgetState>,
    sign_in: SignIn,
    customize: bool,
    drag: Option<CardDrag>,
    /// Each shown card's bounds from the last frame (Customize only).
    card_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    /// The open snooze menu: (widget id, row id).
    snooze_menu: Popup<(String, String)>,
    zeron: Option<ZeronSnapshot>,
    frame: Frame,
    scroll: ScrollHandle,
    save_pending: bool,
    _save: Option<Task<()>>,
    _load: Option<Task<()>>,
    _watch: Option<Task<()>>,
    _path: Option<Task<()>>,
    _sign_in: Option<Task<()>>,
    _zeron_tick: Task<()>,
}

impl EventEmitter<HomeEvent> for Home {}

/// Set only by the real app's boot ([`crate::run_app`]). Without it, as in
/// tests and the example fixtures that build the shell themselves, Home
/// stays empty: it never reads the owner's ~/.keron or Keychain, or reaches
/// the door.
pub(crate) struct OwnerHome;

impl gpui::Global for OwnerHome {}

impl Home {
    /// The owner's Home: `~/.keron`, the door through the login Keychain, a
    /// watched widgets folder, and scripts run with the login shell's PATH.
    /// An empty one unless the app booted as the owner's ([`OwnerHome`]).
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<OwnerHome>() {
            let mut home = Self::bare(
                HomePaths::under_home(std::path::Path::new("/nonexistent")),
                None,
                Some(ScriptOptions::default()),
                cx,
            );
            home.sign_in = SignIn::NoDoor;
            return home;
        }
        let home_dir = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut home = Self::build(
            HomePaths::under_home(&home_dir),
            Some(DoorClient::keron()),
            None,
            cx,
        );
        home.watch_folder = true;
        home.start_login_path(cx);
        home
    }

    /// Home over `paths` with an optional door, no folder watcher, and the
    /// app's own PATH for scripts.
    #[cfg(test)]
    pub(crate) fn with_parts(
        paths: HomePaths,
        door: Option<DoorClient>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(paths, door, Some(ScriptOptions::default()), cx)
    }

    /// `script: None` holds script sources until the login PATH is known.
    fn build(
        paths: HomePaths,
        door: Option<DoorClient>,
        script: Option<ScriptOptions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut home = Self::bare(paths, door, script, cx);
        home.start_load(cx);
        home.check_session(cx);
        home
    }

    /// Home that hasn't loaded anything yet.
    fn bare(
        paths: HomePaths,
        door: Option<DoorClient>,
        script: Option<ScriptOptions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scripts_ready = script.is_some();
        let fetcher = Fetcher::new(door, paths.widgets_dir.clone(), script.unwrap_or_default());
        let zeron_tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ZERON_TICK).await;
                if this
                    .update(cx, |this, cx| this.recompute_zeron(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        cx.on_release(|this, _| this.flush_layout_now()).detach();
        Self {
            paths,
            fetcher,
            scripts_ready,
            watch_folder: false,
            loaded: false,
            catalog: Catalog::default(),
            layout: Layout::default(),
            layout_error: None,
            widgets: HashMap::new(),
            sign_in: SignIn::Checking,
            customize: false,
            drag: None,
            card_bounds: Rc::default(),
            snooze_menu: Popup::default(),
            zeron: None,
            frame: Frame::default(),
            scroll: ScrollHandle::new(),
            save_pending: false,
            _save: None,
            _load: None,
            _watch: None,
            _path: None,
            _sign_in: None,
            _zeron_tick: zeron_tick,
        }
    }

    // ---- from the shell ----

    /// Where to draw: `top` in window coordinates, the composer column's
    /// `width`, and the route fade's `opacity`.
    pub fn set_frame(&mut self, top: f32, width: f32, opacity: f32, cx: &mut Context<Self>) {
        let next = Frame {
            top,
            width,
            opacity,
        };
        let same = (next.top - self.frame.top).abs() < 0.5
            && (next.width - self.frame.width).abs() < 0.5
            && (next.opacity - self.frame.opacity).abs() < 0.002;
        if same {
            return;
        }
        let appeared = self.frame.opacity < 0.5 && opacity >= 0.5;
        self.frame = next;
        if appeared {
            let ids: Vec<String> = self.widgets.keys().cloned().collect();
            for id in ids {
                self.maybe_post_shown(&id, cx);
            }
        }
        cx.notify();
    }

    /// Home is off screen (a chat, Settings): nothing on it counts as seen
    /// until it fades in again.
    pub fn hide(&mut self) {
        self.frame.opacity = 0.0;
    }

    /// The app's own data, for `zeron:` widgets.
    pub fn set_zeron(&mut self, snapshot: ZeronSnapshot, cx: &mut Context<Self>) {
        self.zeron = Some(snapshot);
        self.recompute_zeron(cx);
    }

    /// Whether any shown widget reads `zeron:` data (the shell skips the
    /// snapshot otherwise).
    pub fn wants_zeron(&self) -> bool {
        self.shown_sources()
            .any(|source| matches!(source, SourceSpec::Zeron(_)))
    }

    /// Whether the pull-requests widget shows, so the shell keeps watching.
    pub fn shows_pull_requests(&self) -> bool {
        self.shown_sources()
            .any(|source| matches!(source, SourceSpec::Zeron(name) if name == "pull-requests"))
    }

    fn shown_sources(&self) -> impl Iterator<Item = &SourceSpec> {
        let shown: Vec<&str> = if self.loaded {
            self.layout
                .arrange(&self.catalog.manifests)
                .into_iter()
                .filter(|placed| placed.shown)
                .map(|placed| placed.manifest.id.as_str())
                .collect()
        } else {
            Vec::new()
        };
        self.catalog
            .manifests
            .iter()
            .filter(move |manifest| shown.contains(&manifest.id.as_str()))
            .map(|manifest| &manifest.source)
    }

    // ---- loading and watching ----

    fn start_load(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let load = cx.background_spawn(async move {
            if let Err(error) = catalog::seed(&paths) {
                tracing::warn!(%error, "home: couldn't seed the widgets folder");
            }
            let catalog = catalog::load(&paths);
            let layout = Layout::load(&paths.layout_file).map_err(|error| error.to_string());
            (catalog, layout)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (catalog, layout) = load.await;
            this.update(cx, |this, cx| {
                match layout {
                    Ok(layout) => this.layout = layout,
                    Err(error) => this.layout_error = Some(error),
                }
                this.loaded = true;
                this.apply_catalog(catalog, cx);
                if this.watch_folder {
                    this.start_watching(cx);
                }
            })
            .ok();
        }));
    }

    fn start_watching(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        // The watcher lives on tokio (FSEvents registration blocks); changes
        // come back over the channel. Dropping `_watch` aborts it.
        let watch = Tokio::spawn(cx, async move {
            let mut watcher = match catalog::Watcher::start(&paths).await {
                Ok(watcher) => watcher,
                Err(error) => {
                    tracing::warn!(%error, "home: can't watch the widgets folder");
                    return;
                }
            };
            while watcher.changed().await.is_some() {
                if tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        });
        self._watch = Some(cx.spawn(async move |this, cx| {
            let _watch = watch;
            while rx.next().await.is_some() {
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let load = cx.background_spawn(async move { catalog::load(&paths) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let catalog = load.await;
            this.update(cx, |this, cx| this.apply_catalog(catalog, cx))
                .ok();
        }));
    }

    /// Take a freshly loaded catalog. A widget whose manifest didn't change
    /// keeps its data and refresh loop; a changed one starts over; a removed
    /// one is dropped.
    fn apply_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        let old = std::mem::take(&mut self.catalog.manifests);
        for manifest in &catalog.manifests {
            let unchanged = old
                .iter()
                .any(|previous| previous.id == manifest.id && previous == manifest);
            if !unchanged {
                self.widgets.remove(&manifest.id);
            }
        }
        self.widgets
            .retain(|id, _| catalog.manifests.iter().any(|manifest| &manifest.id == id));
        self.catalog = catalog;
        self.sync_refreshes(cx);
        cx.emit(HomeEvent::LayoutChanged);
        cx.notify();
    }

    /// The login shell's PATH for scripts, found off the main thread (it
    /// may start the owner's shell).
    fn start_login_path(&mut self, cx: &mut Context<Self>) {
        let find = cx.background_spawn(async move { login_shell_path() });
        self._path = Some(cx.spawn(async move |this, cx| {
            let path_env = find.await;
            this.update(cx, |this, cx| {
                let door = this.fetcher.door().cloned();
                let options = ScriptOptions {
                    path_env,
                    ..ScriptOptions::default()
                };
                this.fetcher = Fetcher::new(door, this.paths.widgets_dir.clone(), options);
                this.scripts_ready = true;
                this.sync_refreshes(cx);
            })
            .ok();
        }));
    }

    // ---- refreshing ----

    /// Every widget in Home's order, shown or not.
    fn arranged(&self) -> Vec<Slot> {
        if !self.loaded {
            return Vec::new();
        }
        self.layout
            .arrange(&self.catalog.manifests)
            .into_iter()
            .map(|placed| Slot {
                manifest: placed.manifest.clone(),
                shown: placed.shown,
                width: placed.width,
            })
            .collect()
    }

    fn mode(&self, manifest: &Manifest) -> Mode {
        match &manifest.source {
            SourceSpec::Zeron(_) => Mode::Zeron,
            SourceSpec::Script(_) if self.scripts_ready => Mode::Fetch,
            source if is_door(source) && self.sign_in == SignIn::SignedIn => Mode::Fetch,
            _ => Mode::Wait,
        }
    }

    /// Start a loop for every shown widget that can fetch and has none, and
    /// stop the loops of hidden widgets and of those that can't fetch now.
    fn sync_refreshes(&mut self, cx: &mut Context<Self>) {
        for slot in self.arranged() {
            let id = slot.manifest.id.clone();
            let mode = self.mode(&slot.manifest);
            if !slot.shown || matches!(mode, Mode::Wait) {
                if let Some(state) = self.widgets.get_mut(&id) {
                    state.refresh = None;
                    state.loading = false;
                }
                continue;
            }
            if matches!(mode, Mode::Fetch)
                && self
                    .widgets
                    .get(&id)
                    .is_none_or(|state| state.refresh.is_none())
            {
                self.start_refresh(slot.manifest, cx);
            }
        }
        self.recompute_zeron(cx);
    }

    /// Fetch now, then every `every`, until the widget hides or the door
    /// signs out. Replaces a running loop (Retry, after an action).
    fn start_refresh(&mut self, manifest: Manifest, cx: &mut Context<Self>) {
        let id = manifest.id.clone();
        let fetcher = self.fetcher.clone();
        let every = manifest.every.clamp(
            keron_home::manifest::MIN_EVERY,
            keron_home::manifest::MAX_EVERY,
        );
        let task = cx.spawn({
            let id = id.clone();
            async move |this, cx| {
                loop {
                    let started = Instant::now();
                    let fetch = {
                        let fetcher = fetcher.clone();
                        let manifest = manifest.clone();
                        Tokio::spawn(cx, async move { fetcher.fetch(&manifest).await })
                    };
                    let result = fetch.await.unwrap_or_else(|_| {
                        Err(FetchError::Unsupported("the fetch stopped".to_string()))
                    });
                    let go_on = this
                        .update(cx, |this, cx| this.apply_fetch(&id, started, result, cx))
                        .unwrap_or(false);
                    if !go_on {
                        break;
                    }
                    cx.background_executor().timer(every).await;
                }
            }
        });
        let state = self.widgets.entry(id).or_default();
        state.loading = state.payload.is_none();
        state.refresh = Some(task);
        cx.notify();
    }

    /// A fetch that began at `started` came back. Returns whether the loop
    /// goes on.
    fn apply_fetch(
        &mut self,
        id: &str,
        started: Instant,
        result: Result<Payload, FetchError>,
        cx: &mut Context<Self>,
    ) -> bool {
        if matches!(result, Err(FetchError::SignedOut)) {
            self.signed_out(cx);
            return false;
        }
        let state = self.widgets.entry(id.to_string()).or_default();
        state.loading = false;
        match result {
            Ok(payload) => {
                state.overrides.retain(|_, change| !change.settled);
                state.payload = Some(payload);
                state.error = None;
                if state
                    .action_error
                    .as_ref()
                    .is_some_and(|(_, failed)| *failed < started)
                {
                    state.action_error = None;
                }
            }
            Err(error) => state.error = Some(error.to_string()),
        }
        self.maybe_post_shown(id, cx);
        cx.notify();
        true
    }

    pub(crate) fn retry(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(slot) = self
            .arranged()
            .into_iter()
            .find(|slot| slot.manifest.id == id)
        else {
            return;
        };
        if matches!(self.mode(&slot.manifest), Mode::Fetch) {
            self.start_refresh(slot.manifest, cx);
        }
    }

    fn recompute_zeron(&mut self, cx: &mut Context<Self>) {
        if self.zeron.is_none() {
            return;
        }
        let now_ms = Utc::now().timestamp_millis();
        let mut changed = false;
        for slot in self.arranged() {
            let SourceSpec::Zeron(name) = &slot.manifest.source else {
                continue;
            };
            if !slot.shown {
                continue;
            }
            let Some(snapshot) = self.zeron.as_ref() else {
                return;
            };
            let (payload, unavailable) = match keron_home::zeron::payload(name, snapshot, now_ms) {
                Ok(payload) => (Some(payload), None),
                Err(reason) => (None, Some(reason)),
            };
            let state = self.widgets.entry(slot.manifest.id.clone()).or_default();
            if state.payload != payload || state.unavailable != unavailable || state.loading {
                state.payload = payload;
                state.unavailable = unavailable;
                state.loading = false;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    // ---- the door sign-in ----

    fn check_session(&mut self, cx: &mut Context<Self>) {
        let Some(door) = self.fetcher.door().cloned() else {
            self.sign_in = SignIn::NoDoor;
            return;
        };
        self.sign_in = SignIn::Checking;
        let check = cx.background_spawn(async move { door.has_session() });
        self._sign_in = Some(cx.spawn(async move |this, cx| {
            let has_session = check.await;
            this.update(cx, |this, cx| {
                if this.sign_in == SignIn::Checking {
                    this.sign_in = if has_session {
                        SignIn::SignedIn
                    } else {
                        SignIn::SignedOut
                    };
                    this.sync_refreshes(cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// "Connect your memory": the browser sign-in, then every door widget
    /// fetches again.
    pub(crate) fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(door) = self.fetcher.door().cloned() else {
            return;
        };
        self.sign_in = SignIn::Opening;
        cx.notify();
        self._sign_in = Some(cx.spawn(async move |this, cx| {
            let begin = {
                let door = door.clone();
                Tokio::spawn(cx, async move { door.begin_sign_in().await })
            };
            let pending = match begin.await {
                Ok(Ok(pending)) => pending,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| this.sign_in_failed(error.to_string(), cx))
                        .ok();
                    return;
                }
                Err(_) => {
                    this.update(cx, |this, cx| {
                        this.sign_in_failed("the sign-in stopped".to_string(), cx)
                    })
                    .ok();
                    return;
                }
            };
            let url = pending.authorize_url.clone();
            let opened = this.update(cx, |this, cx| {
                this.sign_in = SignIn::Waiting;
                cx.open_url(&url);
                cx.notify();
            });
            if opened.is_err() {
                return;
            }
            let finish = Tokio::spawn(cx, async move { door.finish_sign_in(pending).await });
            let result = finish.await;
            this.update(cx, |this, cx| match result {
                Ok(Ok(())) => {
                    this.sign_in = SignIn::SignedIn;
                    this.sync_refreshes(cx);
                    cx.notify();
                }
                Ok(Err(error)) => this.sign_in_failed(error.to_string(), cx),
                Err(_) => this.sign_in_failed("the sign-in stopped".to_string(), cx),
            })
            .ok();
        }));
    }

    pub(crate) fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        self._sign_in = None;
        self.sign_in = SignIn::SignedOut;
        cx.notify();
    }

    fn sign_in_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.sign_in = SignIn::Failed(message);
        cx.notify();
    }

    /// The door refused the stored sign-in: door widgets stop and show the
    /// connect state (and forget what they showed).
    fn signed_out(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.sign_in, SignIn::Opening | SignIn::Waiting) {
            self.sign_in = SignIn::SignedOut;
        }
        for slot in self.arranged() {
            if !is_door(&slot.manifest.source) {
                continue;
            }
            if let Some(state) = self.widgets.get_mut(&slot.manifest.id) {
                *state = WidgetState::default();
            }
        }
        cx.notify();
    }

    // ---- loose ends ----

    /// Snooze, done or dismiss one row: shown at once, then sent through the
    /// door, then the widget fetches again. A failure puts the row back and
    /// says why on the card.
    pub(crate) fn act(&mut self, widget: &str, row: &str, action: Action, cx: &mut Context<Self>) {
        let Some(door) = self.fetcher.door().cloned() else {
            return;
        };
        let change = match action {
            Action::Done | Action::Dismiss => RowChange::Hidden,
            Action::Snooze { .. } => RowChange::Snoozed,
            Action::Shown => return,
        };
        let ids = vec![row.to_string()];
        let send = Tokio::spawn(
            cx,
            async move { loose_ends::act(&door, &action, &ids).await },
        );
        let task = cx.spawn({
            let (widget, row) = (widget.to_string(), row.to_string());
            async move |this, cx| {
                let result = send
                    .await
                    .unwrap_or_else(|_| {
                        Err(FetchError::Unsupported("the request stopped".to_string()))
                    })
                    .map(|_| ());
                this.update(cx, |this, cx| this.finish_action(&widget, &row, result, cx))
                    .ok();
            }
        });
        let state = self.widgets.entry(widget.to_string()).or_default();
        state.overrides.insert(
            row.to_string(),
            RowOverride {
                change,
                settled: false,
            },
        );
        state.action_error = None;
        state.actions.insert(row.to_string(), task);
        cx.notify();
    }

    fn finish_action(
        &mut self,
        widget: &str,
        row: &str,
        result: Result<(), FetchError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.widgets.get_mut(widget) else {
            return;
        };
        state.actions.remove(row);
        match result {
            Ok(()) => {
                if let Some(change) = state.overrides.get_mut(row) {
                    change.settled = true;
                }
                self.retry(widget, cx);
            }
            Err(FetchError::SignedOut) => self.signed_out(cx),
            Err(error) => {
                state.overrides.remove(row);
                state.action_error = Some((error.to_string(), Instant::now()));
            }
        }
        cx.notify();
    }

    /// Tell the door which loose ends are on screen: when they first show,
    /// then at most once an hour.
    fn maybe_post_shown(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frame.opacity < 0.5 || self.frame.width <= 0.0 || self.sign_in != SignIn::SignedIn {
            return;
        }
        let Some(door) = self.fetcher.door().cloned() else {
            return;
        };
        let Some(slot) = self
            .arranged()
            .into_iter()
            .find(|slot| slot.shown && slot.manifest.id == id && is_door(&slot.manifest.source))
        else {
            return;
        };
        let Some(state) = self.widgets.get_mut(id) else {
            return;
        };
        if state
            .shown_posted
            .is_some_and(|posted| posted.elapsed() < SHOWN_EVERY)
        {
            return;
        }
        let Some(Payload {
            body: Body::List(items),
            ..
        }) = &state.payload
        else {
            return;
        };
        let (rows, _) = visible_list_rows(items, &state.overrides, limit(&slot.manifest));
        let ids: Vec<String> = rows
            .iter()
            .filter(|row| !row.actions.is_empty())
            .filter_map(|row| row.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        state.shown_posted = Some(Instant::now());
        let post = Tokio::spawn(cx, async move {
            loose_ends::act(&door, &Action::Shown, &ids).await
        });
        state.shown_task = Some(cx.background_spawn(async move {
            if let Ok(Err(error)) = post.await {
                tracing::debug!(%error, "home: couldn't report shown loose ends");
            }
        }));
    }

    pub(crate) fn open_snooze_menu(&mut self, widget: &str, row: &str, cx: &mut Context<Self>) {
        if self.snooze_menu.take_press_was_open() {
            self.close_snooze_menu(cx);
            return;
        }
        self.snooze_menu.open((widget.to_string(), row.to_string()));
        cx.notify();
    }

    pub(crate) fn close_snooze_menu(&mut self, cx: &mut Context<Self>) {
        if self.snooze_menu.begin_close() {
            crate::popover::reap_popup(cx, |home: &mut Self| &mut home.snooze_menu);
        }
        cx.notify();
    }

    // ---- links ----

    /// A row's link: the app's own chats open here, http(s) in the
    /// browser, anything else not at all.
    pub(crate) fn open_link(&mut self, link: &str, cx: &mut Context<Self>) {
        if let Some(chat_id) = parse_chat_link(link) {
            cx.emit(HomeEvent::OpenChat(chat_id.to_string()));
        } else if is_openable(link) {
            cx.open_url(link);
        }
    }

    // ---- Customize ----

    pub(crate) fn toggle_customize(&mut self, cx: &mut Context<Self>) {
        self.customize = !self.customize;
        self.drag = None;
        self.card_bounds.borrow_mut().clear();
        cx.notify();
    }

    pub(crate) fn describe_widget(&mut self, cx: &mut Context<Self>) {
        cx.emit(HomeEvent::DescribeWidget(
            DESCRIBE_WIDGET_STARTER.to_string(),
        ));
    }

    pub(crate) fn set_shown(&mut self, id: &str, shown: bool, cx: &mut Context<Self>) {
        self.layout.set_shown(id, shown, &self.catalog.manifests);
        self.layout_changed(cx);
    }

    /// Move a widget up (`-1`) or down (`1`) in Home's order.
    pub(crate) fn move_by(&mut self, id: &str, delta: isize, cx: &mut Context<Self>) {
        let order = self.arranged();
        let Some(index) = order.iter().position(|slot| slot.manifest.id == id) else {
            return;
        };
        let Some(to) = index
            .checked_add_signed(delta)
            .filter(|to| *to < order.len())
        else {
            return;
        };
        self.layout.move_to(id, to, &self.catalog.manifests);
        self.layout_changed(cx);
    }

    pub(crate) fn set_width(&mut self, id: &str, width: u8, cx: &mut Context<Self>) {
        self.layout.set_width(id, width, &self.catalog.manifests);
        self.layout_changed(cx);
    }

    fn layout_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
        self.sync_refreshes(cx);
        cx.emit(HomeEvent::LayoutChanged);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.layout_error.is_some() {
            return;
        }
        self.save_pending = true;
        self._save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok((layout, path)) = this.update(cx, |this, _| {
                this.save_pending = false;
                (this.layout.clone(), this.paths.layout_file.clone())
            }) else {
                return;
            };
            // Detached, so a write that started still finishes if Home goes.
            cx.background_spawn(async move {
                if let Err(error) = layout.save(&path) {
                    tracing::warn!(%error, "home: couldn't save home.toml");
                }
            })
            .detach();
        }));
    }

    /// A change still waiting for the debounce is written when Home goes.
    fn flush_layout_now(&mut self) {
        if std::mem::take(&mut self.save_pending)
            && self.layout_error.is_none()
            && let Err(error) = self.layout.save(&self.paths.layout_file)
        {
            tracing::warn!(%error, "home: couldn't save home.toml");
        }
    }

    // ---- dragging cards ----

    fn drag_over(
        &mut self,
        id: &str,
        from: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let scroll_y = self.scroll.offset().y;
        if self.drag.as_ref().is_none_or(|drag| drag.id != id) {
            let shown: Vec<String> = self
                .arranged()
                .into_iter()
                .filter(|slot| slot.shown)
                .map(|slot| slot.manifest.id)
                .collect();
            let slots = {
                let measured = self.card_bounds.borrow();
                shown
                    .iter()
                    .map(|id| measured.get(id).copied())
                    .collect::<Option<Vec<_>>>()
            };
            let Some(slots) = slots else {
                return;
            };
            self.drag = Some(CardDrag {
                id: id.to_string(),
                from,
                over: from,
                prev_over: from,
                epoch: 0,
                slots,
                scroll_y,
            });
            cx.notify();
        }
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        // The slots were measured at the drag's scroll position.
        let at = point(position.x, position.y - (scroll_y - drag.scroll_y));
        if let Some(over) = drag.slots.iter().position(|bounds| bounds.contains(&at))
            && over != drag.over
        {
            drag.prev_over = drag.over;
            drag.over = over;
            drag.epoch = drag.epoch.wrapping_add(1);
            cx.notify();
        }
    }

    fn drop_card(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.over != drag.from {
            let order = self.arranged();
            let all: Vec<&str> = order.iter().map(|slot| slot.manifest.id.as_str()).collect();
            let shown: Vec<&str> = order
                .iter()
                .filter(|slot| slot.shown)
                .map(|slot| slot.manifest.id.as_str())
                .collect();
            let index = drop_target(&all, &shown, &drag.id, drag.over);
            self.layout
                .move_to(&drag.id, index, &self.catalog.manifests);
            self.layout_changed(cx);
        }
        cx.notify();
    }

    fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}

fn is_door(source: &SourceSpec) -> bool {
    matches!(source, SourceSpec::KeronSources(_) | SourceSpec::Memory(_))
}

fn limit(manifest: &Manifest) -> usize {
    manifest.limit.unwrap_or(DEFAULT_LIMIT).max(1)
}

/// The rows a list card draws: rows just marked done or "not a thing" are
/// left out, then at most `limit`; and how many more there are.
fn visible_list_rows<'a>(
    items: &'a [ListItem],
    overrides: &HashMap<String, RowOverride>,
    limit: usize,
) -> (Vec<&'a ListItem>, usize) {
    let rows: Vec<&ListItem> = items
        .iter()
        .filter(|item| {
            !item
                .id
                .as_ref()
                .and_then(|id| overrides.get(id))
                .is_some_and(|change| change.change == RowChange::Hidden)
        })
        .collect();
    let more = rows.len().saturating_sub(limit);
    (rows.into_iter().take(limit).collect(), more)
}

/// Where a card dropped at `over` among the shown cards goes in the full
/// order (hidden widgets included): just before the shown card it lands in
/// front of, or just after the last shown card.
fn drop_target(all: &[&str], shown: &[&str], id: &str, over: usize) -> usize {
    let others: Vec<&str> = all.iter().copied().filter(|other| *other != id).collect();
    let shown_others: Vec<&str> = shown.iter().copied().filter(|other| *other != id).collect();
    let position = |target: &str| others.iter().position(|other| *other == target);
    match shown_others.get(over) {
        Some(next) => position(next).unwrap_or(others.len()),
        None => shown_others
            .last()
            .and_then(|last| position(last))
            .map_or(others.len(), |last| last + 1),
    }
}

/// Grid cells, `(row, column)` from 0, for cards of these widths, packed like
/// CSS `grid-auto-flow: row dense`: each card takes the first free cell from
/// the top, so a wide card after an odd one doesn't leave a hole.
fn dense_cells(widths: &[u8], columns: u16) -> Vec<(u16, u16)> {
    let columns = usize::from(columns.max(1));
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let mut cells = Vec::with_capacity(widths.len());
    for &width in widths {
        let span = usize::from(width).clamp(1, columns);
        let mut row = 0;
        loop {
            if taken.len() <= row {
                taken.push(vec![false; columns]);
            }
            let free = (0..=columns - span).find(|&col| (col..col + span).all(|c| !taken[row][c]));
            if let Some(col) = free {
                taken[row][col..col + span].fill(true);
                cells.push((row as u16, col as u16));
                break;
            }
            row += 1;
        }
    }
    cells
}

/// The login shell's PATH joined after the app's own, as the harnesses
/// compose it for the CLIs they start.
fn login_shell_path() -> Option<String> {
    let mut command = tokio::process::Command::new("true");
    zeron_harness::compose_login_shell_path(&mut command);
    command
        .as_std()
        .get_envs()
        .find(|(key, _)| key.to_str() == Some("PATH"))
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}
