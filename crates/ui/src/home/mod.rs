//! Home: the widget grid under the composer on the new-chat screen
//! (docs/plan/04-home.md in the Keron repo).
//!
//! The runtime is the keron-home crate: manifests in `~/.keron/widgets`, the
//! layout in `~/.keron/home.toml`, each kind's JSON, and fetching. This
//! module is the view on top of it. It loads and watches the widgets folder,
//! runs one refresh loop per shown widget, signs in to the door, draws each
//! kind ([`view`]) and owns Customize. The shell hands it the app's own data
//! as a snapshot ([`snapshot`]) and where to sit ([`Home::set_frame`]), and
//! listens for [`HomeEvent`]s. Loose ends the server says are due become Mac
//! banners ([`notices`]).
//!
//! A shown widget with nothing to show (its data is in and empty, nothing
//! wrong) is quiet: unless the owner turned that off, it leaves the grid and
//! sits as an icon chip in the toolbar, where a click peeks at its card.

mod bridge;
mod notices;
mod usage;
mod view;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt as _;
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    Pixels, Point, Role, ScrollHandle, ScrollWheelEvent, Subscription, Task, TouchPhase,
    WeakFocusHandle, Window, point,
};
use gpui_tokio::Tokio;
use keron_door::{DoorClient, DoorError};
use keron_home::catalog::{self, Catalog};
use keron_home::kinds::{is_openable, parse_chat_link};
use keron_home::loose_ends::{self, Action};
use keron_home::script::ScriptOptions;
use keron_home::{
    Body, FetchError, Fetcher, HomePaths, Layout, ListItem, Manifest, Payload, SourceSpec,
    ZeronSnapshot,
};

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::Popup;
use crate::settings::widgets::PageScroll;

pub(crate) use bridge::snapshot;
pub(crate) use usage::{cached_accounts, load_once as load_usage};

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
/// A widget moving between its chip and its card fades in on this.
const SWAP: crate::motion::MotionSpec = crate::motion::DIALOG_IN;
/// A wheel gesture stays with the list or page it started scrolling,
/// momentum included, until the wheel rests this long.
const WHEEL_HOLD: Duration = Duration::from_millis(200);

/// Sends one row action to the door and says whether it took it. Tests use
/// a fake.
pub(crate) type Poster = Rc<dyn Fn(loose_ends::Request, &App) -> Task<Result<(), FetchError>>>;

/// The real poster: through the door client, on the Tokio runtime.
fn door_poster(door: DoorClient) -> Poster {
    Rc::new(move |request, cx| {
        let door = door.clone();
        let send = Tokio::spawn(cx, async move {
            loose_ends::send(&door, &request).await.map(|_| ())
        });
        cx.background_spawn(async move {
            send.await
                .unwrap_or_else(|_| Err(FetchError::Unsupported("the request stopped".to_string())))
        })
    })
}

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
    /// The composer's width above Home: the toolbar ends where it ends.
    composer: f32,
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
    /// Optimistic row changes (snoozed, done, not a thing), by row id.
    overrides: HashMap<String, RowOverride>,
    actions: HashMap<String, Task<()>>,
    /// Why the last action failed, and when. A good fetch that began after
    /// it clears it.
    action_error: Option<(String, Instant)>,
    shown_posted: Option<Instant>,
    shown_task: Option<Task<()>>,
    /// Rows done or "not a thing" a moment ago, still collapsing out of the
    /// card, by row id.
    leaving: HashMap<String, Leaving>,
    /// What each row with actions, or with a link in a card that scrolls,
    /// keeps between frames, by row id.
    row_ui: Rc<RefCell<HashMap<String, RowUi>>>,
}

impl WidgetState {
    /// What `row` keeps between frames, made the first time it's drawn.
    fn row_ui(&self, row: &str, cx: &mut App) -> RefMut<'_, RowUi> {
        RefMut::map(self.row_ui.borrow_mut(), |rows| {
            rows.entry(row.to_string())
                .or_insert_with(|| RowUi::new(cx))
        })
    }
}

/// A hidden row on its way out: it fades and its height closes over
/// [`crate::motion::COLLAPSE`], then it's dropped.
struct Leaving {
    started: Instant,
    /// The row's height when it was last drawn whole.
    height: f32,
    _done: Task<()>,
}

impl Leaving {
    /// Eased progress of the collapse, 0 to 1.
    fn progress(&self) -> f32 {
        let total = leave_duration().as_secs_f32();
        crate::motion::COLLAPSE.progress(self.started.elapsed().as_secs_f32() / total)
    }
}

fn leave_duration() -> Duration {
    crate::motion::COLLAPSE
        .total()
        .mul_f32(crate::motion::speed_scale())
}

/// A row between frames: the focus around its action buttons (keyboard
/// focus there shows them, like hover does), the row's own focus when it
/// opens a link, its Snooze button's (focus comes back there from its
/// menu and note field), and where a row with actions was last drawn, in
/// window coordinates (a collapse starts from its height; tooltips sit
/// above it).
struct RowUi {
    focus: FocusHandle,
    link: FocusHandle,
    snooze: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
}

impl RowUi {
    fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            link: cx.focus_handle().tab_stop(true),
            snooze: cx.focus_handle().tab_stop(true),
            bounds: None,
        }
    }
}

/// A list that scrolls inside its card ([`scrolls`]), between frames: its
/// scroll and floating scrollbar, where it was before the wheel event being
/// handled moved it, and what keyboard focus was on (a row's link, Snooze
/// or Done) when it last brought a row into view.
#[derive(Default)]
struct ListScroll {
    page: PageScroll,
    before: Rc<Cell<Pixels>>,
    revealed: Option<WeakFocusHandle>,
}

impl ListScroll {
    /// Bring the row keyboard focus is in (its place among the rows, and
    /// what in it has focus: its link, Snooze or Done) into view each time
    /// focus moves, within the row too; between moves the owner can still
    /// scroll away from it.
    fn reveal(&mut self, focused: Option<(usize, &WeakFocusHandle)>) {
        let Some((ix, handle)) = focused else {
            self.revealed = None;
            return;
        };
        if self.revealed.as_ref() != Some(handle) {
            self.revealed = Some(handle.clone());
            self.page.scroll.scroll_to_item(ix);
        }
    }
}

/// What a wheel gesture scrolls.
#[derive(Clone, Debug, PartialEq)]
enum WheelTarget {
    /// A card's list, by widget id.
    List(String),
    Page,
}

/// Who has the wheel. What a gesture started scrolling keeps it, momentum
/// included, so a flick that runs a card's list to its end doesn't carry on
/// into the page, and a page scroll doesn't catch on a card that comes under
/// the pointer.
#[derive(Default)]
struct Wheel {
    held: Option<(WheelTarget, Instant)>,
}

impl Wheel {
    /// Whether `target` takes a wheel event that it `can_move` with, or
    /// not; `starts` marks a new trackpad gesture. With nothing held (a new
    /// gesture, or the wheel rested for [`WHEEL_HOLD`]) it takes the event
    /// if it can move, and holds the gesture from then on.
    fn takes(&mut self, target: WheelTarget, can_move: bool, starts: bool, now: Instant) -> bool {
        let held = self
            .held
            .take()
            .filter(|(_, at)| !starts && now.saturating_duration_since(*at) < WHEEL_HOLD)
            .map(|(held, _)| held);
        let takes = held.as_ref().map_or(can_move, |held| *held == target);
        // Every event of the gesture keeps it held, taken here or not.
        self.held = match held {
            Some(held) => Some((held, now)),
            None => takes.then_some((target, now)),
        };
        takes
    }
}

/// Which way a loose end closes with the owner's note.
#[derive(Clone, Copy, Debug, PartialEq)]
enum NoteKind {
    Done,
    /// "Not a thing".
    Dismiss,
}

impl NoteKind {
    fn action(self, comment: Option<String>) -> Action {
        match self {
            NoteKind::Done => Action::Done { comment },
            NoteKind::Dismiss => Action::Dismiss { comment },
        }
    }

    /// The field's placeholder, which is also its label.
    fn placeholder(self) -> &'static str {
        match self {
            NoteKind::Done => "How it was handled",
            NoteKind::Dismiss => "Why it's not a thing",
        }
    }
}

/// The one-line field open under a loose end after "Done with a note…" or
/// "Not a thing, because…". Enter closes the row with what was typed, the
/// door adding it to the memory's note; Escape or focus leaving drops it.
/// Typing stops at what the door takes, so Enter always sends.
struct RowNote {
    widget: String,
    row: String,
    kind: NoteKind,
    input: Entity<ComposerInput>,
    /// Focus the field on the next frame, which has the window.
    focus_pending: bool,
    /// A card that scrolls brings the row and its field into view once.
    reveal_pending: Cell<bool>,
    _events: Subscription,
    _blur: Option<Subscription>,
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

/// Home spreads past the composer, up to this wide.
const MAX_WIDTH: f32 = 1320.0;
/// Kept free on each side of Home in the conversation column.
const SIDE_MARGIN: f32 = 32.0;

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
    /// Quiet widgets opened as cards from their chips, this session only.
    peeks: HashSet<String>,
    /// What the last [`Home::sync_quiet`] saw: shown widgets in the chip row,
    /// and those in the grid.
    chip_row: HashSet<String>,
    in_grid: HashSet<String>,
    /// Chips that just appeared for a card that went quiet, and cards that
    /// just appeared for a chip, with when: each fades in on [`SWAP`].
    chip_in: HashMap<String, Instant>,
    card_in: HashMap<String, Instant>,
    drag: Option<CardDrag>,
    /// Each shown card's bounds from the last frame, as drawn: Customize's
    /// drag hit-tests them.
    card_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    /// Each shown card's height from the last frame, without its row's
    /// stretch: these lay out the grid, so a row shrinks with its tallest card.
    card_heights: Rc<RefCell<HashMap<String, f32>>>,
    /// The open snooze menu: (widget id, row id).
    snooze_menu: Popup<(String, String)>,
    /// The snooze menu's card, which has focus when the menu opened from
    /// the keyboard, and the item its arrow keys are on then.
    menu_focus: FocusHandle,
    menu_active: Option<usize>,
    /// The note being written under a loose end, if one is open.
    note: Option<RowNote>,
    /// Notes sent with a Done or "not a thing" the door hasn't taken yet,
    /// by (widget id, row id): if it fails, the row's field opens with the
    /// note again.
    sent_notes: HashMap<(String, String), String>,
    /// The row whose action buttons have keyboard focus: (widget id, row
    /// id), read at the start of each render.
    keyboard_row: Option<(String, String)>,
    /// The row keyboard focus is in, by its link or its action buttons; a
    /// card that scrolls brings it into view.
    keyboard_in: Option<(String, String)>,
    /// What had focus when Home last drew, and whether the keyboard put it
    /// there. A click leaves focus on a button; a later key doesn't make
    /// that keyboard focus.
    focus_seen: Option<(WeakFocusHandle, bool)>,
    zeron: Option<ZeronSnapshot>,
    /// Loose-ends banners; only the owner's Home posts them.
    notices: Option<notices::Notices>,
    /// Row actions go out through this; `None` without a door, and then
    /// rows offer none.
    poster: Option<Poster>,
    frame: Frame,
    scroll: ScrollHandle,
    /// Where Home's page was before the wheel event being handled moved it.
    page_before: Rc<Cell<Pixels>>,
    /// Lists that scroll inside their cards, by widget id.
    list_scrolls: RefCell<HashMap<String, ListScroll>>,
    wheel: Wheel,
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
        let door = DoorClient::keron();
        let mut home = Self::build(
            HomePaths::under_home(&home_dir),
            Some(door.clone()),
            None,
            cx,
        );
        home.notices = Some(notices::Notices::owner(door));
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
        let poster = door.clone().map(door_poster);
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
            peeks: HashSet::new(),
            chip_row: HashSet::new(),
            in_grid: HashSet::new(),
            chip_in: HashMap::new(),
            card_in: HashMap::new(),
            drag: None,
            card_bounds: Rc::default(),
            card_heights: Rc::default(),
            snooze_menu: Popup::default(),
            menu_focus: cx.focus_handle(),
            menu_active: None,
            note: None,
            sent_notes: HashMap::new(),
            keyboard_row: None,
            keyboard_in: None,
            focus_seen: None,
            zeron: None,
            notices: None,
            poster,
            frame: Frame::default(),
            scroll: ScrollHandle::new(),
            page_before: Rc::default(),
            list_scrolls: RefCell::default(),
            wheel: Wheel::default(),
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

    /// Home's width in a conversation column `column` wide: the column less its
    /// margins, up to MAX_WIDTH, and never narrower than the composer above it.
    pub fn frame_width(column: f32, composer: f32) -> f32 {
        (column - 2.0 * SIDE_MARGIN).min(MAX_WIDTH).max(composer)
    }

    /// Where to draw: `top` in window coordinates, Home's `width`
    /// ([`Home::frame_width`]), the `composer`'s width above it, and the
    /// route fade's `opacity`.
    pub fn set_frame(
        &mut self,
        top: f32,
        width: f32,
        composer: f32,
        opacity: f32,
        cx: &mut Context<Self>,
    ) {
        let next = Frame {
            top,
            width,
            composer,
            opacity,
        };
        let same = (next.top - self.frame.top).abs() < 0.5
            && (next.width - self.frame.width).abs() < 0.5
            && (next.composer - self.frame.composer).abs() < 0.5
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

    /// Whether any widget is drawn as a card: the shell then lifts the
    /// new-chat composer so the first cards are in view. Chips alone fit in
    /// the toolbar and leave the composer where it was.
    pub fn has_shown_widget(&self) -> bool {
        !self.grid_slots().is_empty()
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

    /// Whether a `zeron:usage` widget shows, so the shell hands Home the
    /// accounts list (and loads one if nothing has yet).
    pub fn shows_usage(&self) -> bool {
        self.shown_sources()
            .any(|source| matches!(source, SourceSpec::Zeron(name) if name == "usage"))
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
            if let Err(error) = catalog::refresh_readme(&paths) {
                tracing::warn!(%error, "home: couldn't refresh the widgets README");
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
        let fresh = result.is_ok();
        match result {
            Ok(payload) => {
                state.overrides.retain(|_, change| !change.settled);
                let ids: Vec<&str> = match &payload.body {
                    Body::List(items) => {
                        items.iter().filter_map(|item| item.id.as_deref()).collect()
                    }
                    _ => Vec::new(),
                };
                state
                    .row_ui
                    .borrow_mut()
                    .retain(|id, _| ids.contains(&id.as_str()));
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
        if fresh {
            self.post_due_notices(id, started, cx);
        }
        self.sync_quiet(cx);
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
            self.sync_quiet(cx);
            cx.notify();
        }
    }

    // ---- quiet widgets ----

    /// Whether a widget has nothing to show: its data is in and has no rows
    /// (a stat, no value), and nothing about it wants a look (an error, a
    /// first load, the door's sign-in, a row action in flight or closing
    /// up). Hidden or not.
    fn is_quiet(&self, manifest: &Manifest) -> bool {
        if is_door(&manifest.source) && self.sign_in != SignIn::SignedIn {
            return false;
        }
        if matches!(manifest.source, SourceSpec::Script(_)) && !self.scripts_ready {
            return false;
        }
        let Some(state) = self.widgets.get(&manifest.id) else {
            return false;
        };
        let Some(payload) = &state.payload else {
            return false;
        };
        state.error.is_none()
            && state.unavailable.is_none()
            && !state.loading
            && state.action_error.is_none()
            && state.overrides.is_empty()
            && state.leaving.is_empty()
            && payload.errors.is_empty()
            && shows_nothing(&payload.body)
    }

    /// Whether quiet widgets leave the grid right now: the owner's switch,
    /// and never in Customize, where every card can be moved and resized.
    fn collapses(&self) -> bool {
        self.layout.collapse_empty && !self.customize
    }

    /// Shown widgets drawn as chips beside Customize, in Home's order. A
    /// peeked one is in the grid too.
    fn chip_slots(&self) -> Vec<Slot> {
        if !self.collapses() {
            return Vec::new();
        }
        self.arranged()
            .into_iter()
            .filter(|slot| slot.shown && self.is_quiet(&slot.manifest))
            .collect()
    }

    /// Shown widgets drawn as cards, in Home's order.
    fn grid_slots(&self) -> Vec<Slot> {
        let collapses = self.collapses();
        self.arranged()
            .into_iter()
            .filter(|slot| {
                slot.shown
                    && !(collapses
                        && !self.peeks.contains(&slot.manifest.id)
                        && self.is_quiet(&slot.manifest))
            })
            .collect()
    }

    /// Open a quiet widget's card from its chip, or put it back.
    pub(crate) fn toggle_peek(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.peeks.remove(id) {
            self.peeks.insert(id.to_string());
        }
        self.sync_quiet(cx);
        cx.notify();
    }

    pub(crate) fn set_collapse_empty(&mut self, on: bool, cx: &mut Context<Self>) {
        self.layout.collapse_empty = on;
        self.layout_changed(cx);
        self.sync_quiet(cx);
    }

    /// Bring chips and cards in line with the data: a widget that has
    /// something again loses its peek, and whatever just moved between the
    /// chip row and the grid fades in where it lands (not under Reduce
    /// Motion). The quiet state follows the rows, so it only flips when
    /// their count crosses zero, not on every refresh.
    fn sync_quiet(&mut self, cx: &App) {
        let quiet: HashSet<String> = self
            .arranged()
            .into_iter()
            .filter(|slot| slot.shown && self.is_quiet(&slot.manifest))
            .map(|slot| slot.manifest.id)
            .collect();
        self.peeks.retain(|id| quiet.contains(id));
        let chip_row: HashSet<String> = self
            .chip_slots()
            .into_iter()
            .map(|slot| slot.manifest.id)
            .collect();
        let in_grid: HashSet<String> = self
            .grid_slots()
            .into_iter()
            .map(|slot| slot.manifest.id)
            .collect();
        let now = Instant::now();
        if !crate::motion::reduced_motion(cx) {
            for id in chip_row.difference(&self.chip_row) {
                if self.in_grid.contains(id) {
                    self.chip_in.insert(id.clone(), now);
                }
            }
            for id in in_grid.difference(&self.in_grid) {
                if self.chip_row.contains(id) {
                    self.card_in.insert(id.clone(), now);
                }
            }
        }
        let total = swap_duration();
        self.chip_in
            .retain(|id, at| chip_row.contains(id) && at.elapsed() < total);
        self.card_in
            .retain(|id, at| in_grid.contains(id) && at.elapsed() < total);
        self.chip_row = chip_row;
        self.in_grid = in_grid;
    }

    /// How far a chip or card that just appeared has faded in, 0 to 1;
    /// `None` once it's done.
    fn swap_t(started: Option<&Instant>) -> Option<f32> {
        let elapsed = started?.elapsed();
        let total = swap_duration();
        (elapsed < total).then(|| SWAP.progress(elapsed.as_secs_f32() / total.as_secs_f32()))
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
        self.sync_quiet(cx);
        cx.notify();
    }

    // ---- row actions ----

    /// Snooze, done or dismiss one row: shown at once, then sent to the
    /// widget's own source through the door (`/memory/loose-ends/done`,
    /// `/sources/slack-waiting/done`), then the widget fetches again. A
    /// failure puts the row back and says why on the card, except a mail or
    /// Slack row the source has already dropped, which stays hidden.
    pub(crate) fn act(&mut self, widget: &str, row: &str, action: Action, cx: &mut Context<Self>) {
        let Some(post) = self.poster.clone() else {
            return;
        };
        let change = match action {
            Action::Done { .. } | Action::Dismiss { .. } => RowChange::Hidden,
            Action::Snooze { .. } => RowChange::Snoozed,
            Action::Shown | Action::Notified { .. } => return,
        };
        // A row on its way out can still be clicked while it collapses; it's
        // hidden already, so that does nothing.
        let hidden = self
            .widgets
            .get(widget)
            .and_then(|state| state.overrides.get(row))
            .is_some_and(|change| change.change == RowChange::Hidden);
        if hidden {
            return;
        }
        let Some(manifest) = self.catalog.manifests.iter().find(|m| m.id == widget) else {
            return;
        };
        let Ok(mut request) = loose_ends::request(&manifest.source, &action, &[row.to_string()])
        else {
            return;
        };
        // Mail and Slack: say which message the owner saw, so a newer one isn't hidden too.
        if matches!(manifest.source, SourceSpec::KeronSources(_)) {
            let shown = self
                .widgets
                .get(widget)
                .and_then(|state| state.payload.as_ref())
                .and_then(|payload| match &payload.body {
                    Body::List(items) => items
                        .iter()
                        .find(|item| item.id.as_deref() == Some(row))
                        .and_then(|item| item.at.clone()),
                    _ => None,
                });
            if let Some(at) = shown {
                request = request.seen([(row, at.as_str())]);
            }
        }
        if let Action::Done {
            comment: Some(comment),
        }
        | Action::Dismiss {
            comment: Some(comment),
        } = &action
        {
            self.sent_notes
                .insert((widget.to_string(), row.to_string()), comment.clone());
        }
        let send = post(request, cx);
        let task = cx.spawn({
            let (widget, row) = (widget.to_string(), row.to_string());
            async move |this, cx| {
                let result = send.await;
                this.update(cx, |this, cx| this.finish_action(&widget, &row, result, cx))
                    .ok();
            }
        });
        let reduced = crate::motion::reduced_motion(cx);
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
        // A hidden row closes up rather than vanishing, unless Reduce Motion
        // is on or it was never drawn.
        let height = state
            .row_ui
            .borrow()
            .get(row)
            .and_then(|ui| ui.bounds)
            .map(|bounds| f32::from(bounds.size.height))
            .filter(|_| change == RowChange::Hidden && !reduced);
        if let Some(height) = height {
            let done = cx.spawn({
                let (widget, row) = (widget.to_string(), row.to_string());
                async move |this, cx| {
                    cx.background_executor().timer(leave_duration()).await;
                    this.update(cx, |this, cx| this.finish_leaving(&widget, &row, cx))
                        .ok();
                }
            });
            state.leaving.insert(
                row.to_string(),
                Leaving {
                    started: Instant::now(),
                    height,
                    _done: done,
                },
            );
        }
        cx.notify();
    }

    /// A hidden row finished collapsing: drop it, and the next one joins.
    fn finish_leaving(&mut self, widget: &str, row: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.widgets.get_mut(widget)
            && state.leaving.remove(row).is_some()
        {
            cx.notify();
        }
    }

    fn finish_action(
        &mut self,
        widget: &str,
        row: &str,
        result: Result<(), FetchError>,
        cx: &mut Context<Self>,
    ) {
        // A mail or Slack row the poller has already dropped (answered
        // elsewhere) needs no reply: the door's 404 says what the owner
        // wanted is already true, so the next fetch confirms it's gone.
        let result = match result {
            Err(error) if already_gone(&error) && self.from_keron_sources(widget) => Ok(()),
            other => other,
        };
        let Some(state) = self.widgets.get_mut(widget) else {
            return;
        };
        state.actions.remove(row);
        match result {
            Ok(()) => {
                if let Some(change) = state.overrides.get_mut(row) {
                    change.settled = true;
                    if change.change == RowChange::Hidden {
                        self.sent_notes
                            .remove(&(widget.to_string(), row.to_string()));
                    }
                }
                self.retry(widget, cx);
            }
            Err(FetchError::SignedOut) => self.signed_out(cx),
            Err(error) => {
                state.overrides.remove(row);
                state.leaving.remove(row);
                state.action_error = Some((error.to_string(), Instant::now()));
            }
        }
        cx.notify();
    }

    fn from_keron_sources(&self, widget: &str) -> bool {
        self.catalog
            .manifests
            .iter()
            .any(|m| m.id == widget && matches!(m.source, SourceSpec::KeronSources(_)))
    }

    /// Tell the door which loose ends are on screen: when they first show,
    /// then at most once an hour.
    fn maybe_post_shown(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.frame.opacity < 0.5 || self.frame.width <= 0.0 || self.sign_in != SignIn::SignedIn {
            return;
        }
        let Some(post) = self.poster.clone() else {
            return;
        };
        let loose_ends = loose_ends::loose_ends();
        let Some(slot) = self.arranged().into_iter().find(|slot| {
            slot.shown && slot.manifest.id == id && slot.manifest.source == loose_ends
        }) else {
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
        // The rows in view before the card scrolls, if it does.
        let (rows, _) = visible_list_rows(items, &state.overrides, limit(&slot.manifest));
        let ids: Vec<String> = rows
            .iter()
            .filter(|row| !row.actions.is_empty())
            .filter_map(|row| row.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let Ok(request) = loose_ends::request(&loose_ends, &Action::Shown, &ids) else {
            return;
        };
        state.shown_posted = Some(Instant::now());
        let post = post(request, cx);
        state.shown_task = Some(cx.background_spawn(async move {
            if let Err(error) = post.await {
                tracing::debug!(%error, "home: couldn't report shown loose ends");
            }
        }));
    }

    /// Open a row's snooze menu. From the keyboard, its card takes focus
    /// with the first item picked, for the arrow keys and Enter.
    pub(crate) fn open_snooze_menu(
        &mut self,
        widget: &str,
        row: &str,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snooze_menu.take_press_was_open() {
            self.close_snooze_menu(cx);
            return;
        }
        self.snooze_menu.open((widget.to_string(), row.to_string()));
        self.menu_active = from_keyboard.then_some(0);
        if from_keyboard {
            window.focus(&self.menu_focus, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_snooze_menu(&mut self, cx: &mut Context<Self>) {
        if self.snooze_menu.begin_close() {
            crate::popover::reap_popup(cx, |home: &mut Self| &mut home.snooze_menu);
        }
        cx.notify();
    }

    /// Give focus back to a row's Snooze button.
    fn focus_snooze(&self, widget: &str, row: &str, window: &mut Window, cx: &mut App) {
        let snooze = self.widgets.get(widget).and_then(|state| {
            let rows = state.row_ui.borrow();
            rows.get(row).map(|ui| ui.snooze.clone())
        });
        if let Some(snooze) = snooze {
            window.focus(&snooze, cx);
        }
    }

    /// Open the note field under a loose end, focused from the next frame,
    /// with the note the door last failed to take on that row, if any.
    fn open_note(&mut self, widget: &str, row: &str, kind: NoteKind, cx: &mut Context<Self>) {
        let unsent = self
            .sent_notes
            .remove(&(widget.to_string(), row.to_string()));
        let input = cx.new(|cx| {
            let mut input = ComposerInput::new(kind.placeholder(), cx)
                .with_single_line()
                .with_max_chars(loose_ends::COMMENT_LIMIT)
                .with_text_metrics(12.5, 16.0)
                .with_accessibility_role(Role::TextInput);
            if let Some(unsent) = unsent {
                input.set_text(unsent, cx);
            }
            input
        });
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.send_note(cx),
            // Its length shows as it nears what the door takes.
            ComposerInputEvent::Edited => cx.notify(),
            _ => {}
        });
        self.note = Some(RowNote {
            widget: widget.to_string(),
            row: row.to_string(),
            kind,
            input,
            focus_pending: true,
            reveal_pending: Cell::new(true),
            _events: events,
            _blur: None,
        });
        cx.notify();
    }

    /// Focus a just-opened note field, and drop it once focus leaves: a
    /// click away, or Tab. Switching to another app keeps it, so the owner
    /// can look something up and come back to finish it.
    fn focus_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.note.as_mut() else {
            return;
        };
        if !std::mem::take(&mut note.focus_pending) {
            return;
        }
        let focus = note.input.focus_handle(cx);
        window.focus(&focus, cx);
        note._blur = Some(cx.on_blur(&focus, window, |this, window, cx| {
            if window.is_window_active() {
                this.cancel_note(cx);
            }
        }));
    }

    /// Enter in the note field: the row closes as Done or "not a thing",
    /// with the note, or without one when the field is empty.
    fn send_note(&mut self, cx: &mut Context<Self>) {
        let Some(note) = self.note.take() else {
            return;
        };
        let comment = loose_ends::clean_comment(note.input.read(cx).text());
        self.act(&note.widget, &note.row, note.kind.action(comment), cx);
    }

    fn cancel_note(&mut self, cx: &mut Context<Self>) {
        if self.note.take().is_some() {
            cx.notify();
        }
    }

    /// Escape in the note field: drop it, and give focus back to the row's
    /// Snooze, whose menu opened it.
    fn escape_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.note.take() else {
            return;
        };
        self.focus_snooze(&note.widget, &note.row, window, cx);
        cx.notify();
    }

    /// Which rows have focus from the keyboard: the one whose action buttons
    /// have it ([`Home::keyboard_row`]: they show, as on hover), and the one
    /// it's in at all, by its link or its buttons ([`Home::keyboard_in`]).
    /// Focus a click left doesn't count, even after a key is pressed
    /// somewhere else.
    fn note_keyboard_focus(&mut self, window: &Window, cx: &App) {
        let focused = window.focused(cx);
        let by_keyboard = match (&focused, &self.focus_seen) {
            (Some(handle), Some((seen, by_keyboard))) if seen == handle => *by_keyboard,
            (Some(_), _) => window.last_input_was_keyboard(),
            (None, _) => false,
        };
        self.focus_seen = focused.map(|handle| (handle.downgrade(), by_keyboard));
        let keyboard = by_keyboard && window.last_input_was_keyboard();
        let find = |has_focus: &dyn Fn(&RowUi) -> bool| {
            if !keyboard {
                return None;
            }
            self.widgets.iter().find_map(|(widget, state)| {
                state
                    .row_ui
                    .borrow()
                    .iter()
                    .find(|(_, ui)| has_focus(ui))
                    .map(|(row, _)| (widget.clone(), row.clone()))
            })
        };
        let actions = find(&|ui| ui.focus.contains_focused(window, cx));
        let link = find(&|ui| ui.link.is_focused(window));
        self.keyboard_in = actions.clone().or(link);
        self.keyboard_row = actions;
    }

    // ---- scrolling ----

    fn list_scroll(&mut self, widget: &str) -> &mut ListScroll {
        self.list_scrolls
            .get_mut()
            .entry(widget.to_string())
            .or_default()
    }

    /// Route a wheel event that `target`'s own scroll listener has just
    /// moved it by: it keeps the move when it takes the event ([`Wheel`]),
    /// and undoes it otherwise. Returns whether it took it.
    fn settle_wheel(&mut self, target: WheelTarget, event: &ScrollWheelEvent) -> bool {
        let (scroll, before) = match &target {
            WheelTarget::Page => (self.scroll.clone(), self.page_before.get()),
            WheelTarget::List(widget) => {
                let list = self.list_scroll(widget);
                (list.page.scroll.clone(), list.before.get())
            }
        };
        let offset = scroll.offset();
        let max = f32::from(scroll.max_offset().y).max(0.0);
        let after = f32::from(offset.y).clamp(-max, 0.0);
        let can_move = (after - f32::from(before)).abs() > 0.01;
        let starts = event.touch_phase == TouchPhase::Started;
        let takes = self.wheel.takes(target, can_move, starts, Instant::now());
        if !takes {
            scroll.set_offset(point(offset.x, before));
        }
        takes
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
        self.sync_quiet(cx);
        cx.notify();
    }

    pub(crate) fn describe_widget(&mut self, cx: &mut Context<Self>) {
        cx.emit(HomeEvent::DescribeWidget(
            DESCRIBE_WIDGET_STARTER.to_string(),
        ));
    }

    /// Built-ins with no file in the widgets folder, as `(id, title)`.
    pub(crate) fn missing_builtins(&self) -> Vec<(&'static str, String)> {
        if !self.loaded {
            return Vec::new();
        }
        catalog::missing_builtins(&self.catalog)
    }

    /// Write a missing built-in's manifest into the widgets folder, then
    /// load the folder again (the watcher would too, but tests have none).
    pub(crate) fn add_builtin(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let write = cx.background_spawn(async move {
            let added = catalog::add_builtin(&paths, id);
            (added, catalog::load(&paths))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (added, catalog) = write.await;
            if let Err(error) = added {
                tracing::warn!(%error, "home: couldn't add a built-in widget");
            }
            this.update(cx, |this, cx| this.apply_catalog(catalog, cx))
                .ok();
        }));
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
                .grid_slots()
                .into_iter()
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
            let grid = self.grid_slots();
            let all: Vec<&str> = order.iter().map(|slot| slot.manifest.id.as_str()).collect();
            let shown: Vec<&str> = grid.iter().map(|slot| slot.manifest.id.as_str()).collect();
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

/// The door's answer for an action on a row it no longer has.
fn already_gone(error: &FetchError) -> bool {
    matches!(
        error,
        FetchError::Door(DoorError::Http { status: 404, error: Some(code), .. }) if code == "unknown_item"
    )
}

fn is_door(source: &SourceSpec) -> bool {
    matches!(source, SourceSpec::KeronSources(_) | SourceSpec::Memory(_))
}

fn swap_duration() -> Duration {
    SWAP.total().mul_f32(crate::motion::speed_scale())
}

/// Whether a widget's data has nothing to draw: no rows, or a stat with
/// neither a value nor bars.
fn shows_nothing(body: &Body) -> bool {
    match body {
        Body::Stat(stat) => stat.value.is_empty() && stat.series.is_empty(),
        body => body.is_empty(),
    }
}

fn limit(manifest: &Manifest) -> usize {
    manifest.limit.unwrap_or(DEFAULT_LIMIT).max(1)
}

/// Whether a card scrolls through every row rather than stopping at its
/// limit with "+N more". Loose ends do, so nothing open is out of reach, and
/// so does the memory's timeline of today; the limit then sets how many rows
/// tall the card is.
fn scrolls(manifest: &Manifest) -> bool {
    manifest.source == loose_ends::loose_ends()
        || manifest.source == SourceSpec::Memory("today".to_string())
}

/// The most rows a list card draws: all of them when it scrolls.
fn drawn_limit(manifest: &Manifest) -> usize {
    if scrolls(manifest) {
        usize::MAX
    } else {
        limit(manifest)
    }
}

/// The rows a list card draws: rows just marked done or "not a thing" are
/// left out, then at most `limit`; and how many more there are.
fn visible_list_rows<'a>(
    items: &'a [ListItem],
    overrides: &HashMap<String, RowOverride>,
    limit: usize,
) -> (Vec<&'a ListItem>, usize) {
    let drawn = drawn_list_rows(items, overrides, |_| false, limit);
    (
        drawn.rows.into_iter().map(|(row, _)| row).collect(),
        drawn.more,
    )
}

/// How a list row is drawn while rows close up after Done.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drawn<'a> {
    Whole,
    /// Hidden a moment ago and closing up.
    Leaving,
    /// Past the limit until the leaving row `with` gave up its place: it
    /// opens as that row closes, so the card keeps its height.
    Joining {
        with: &'a str,
    },
}

/// What a list card draws.
#[derive(Debug, PartialEq)]
struct DrawnList<'a> {
    rows: Vec<(&'a ListItem, Drawn<'a>)>,
    /// Rows past the limit.
    more: usize,
    /// The "+N more" line closing up, with the count it showed: the rows
    /// past the limit all joined while rows above them leave.
    more_closing: Option<usize>,
}

/// The rows a list card draws: as [`visible_list_rows`], except that a
/// hidden row still collapsing keeps its place until it's gone. It doesn't
/// hold a place under the limit, so the next row joins as it closes.
fn drawn_list_rows<'a>(
    items: &'a [ListItem],
    overrides: &HashMap<String, RowOverride>,
    leaving: impl Fn(&str) -> bool,
    limit: usize,
) -> DrawnList<'a> {
    let kept: Vec<(&ListItem, Option<&str>)> = items
        .iter()
        .filter_map(|item| {
            let id = item.id.as_deref();
            let hidden = id
                .and_then(|id| overrides.get(id))
                .is_some_and(|change| change.change == RowChange::Hidden);
            if !hidden {
                return Some((item, None));
            }
            id.filter(|id| leaving(id)).map(|id| (item, Some(id)))
        })
        .collect();
    let whole_total = kept.iter().filter(|(_, leaving)| leaving.is_none()).count();
    let mut rows = Vec::new();
    let mut whole = 0;
    let mut leavers: Vec<&str> = Vec::new();
    let mut joined = 0;
    for (position, &(item, leaving)) in kept.iter().enumerate() {
        if whole == limit {
            break;
        }
        // Drawn before the leaving rows started to go: under the limit.
        let was_drawn = position < limit;
        match leaving {
            Some(id) if was_drawn => {
                leavers.push(id);
                rows.push((item, Drawn::Leaving));
            }
            Some(_) => {}
            None => {
                whole += 1;
                // Each row that joins takes the place of one leaving above it.
                let with = (!was_drawn).then(|| leavers.get(joined)).flatten();
                joined += usize::from(with.is_some());
                let drawn = with.map_or(Drawn::Whole, |&with| Drawn::Joining { with });
                rows.push((item, drawn));
            }
        }
    }
    let more = whole_total.saturating_sub(limit);
    let shown_before = kept.len().saturating_sub(limit);
    DrawnList {
        rows,
        more,
        more_closing: (more == 0 && shown_before > 0).then_some(shown_before),
    }
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

/// A card's place in Home's grid, in pixels from the grid's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    x: f32,
    y: f32,
    width: f32,
    /// Its row's height: the tallest card's in it.
    height: f32,
}

/// Where each card goes in a grid `width` wide and `columns` across, and the
/// grid's height. In order, each card takes its `widths` in columns in the
/// current row, or starts the next row when they don't fit. A row's cards
/// share its whole width by their columns, and all are as tall as its
/// tallest (`heights`), so their edges line up.
fn pack_rows(
    widths: &[u8],
    heights: &[f32],
    columns: u16,
    width: f32,
    gap: f32,
) -> (Vec<Place>, f32) {
    let columns = columns.max(1);
    let spans: Vec<u16> = widths
        .iter()
        .map(|&span| u16::from(span).clamp(1, columns))
        .collect();
    let mut places = Vec::with_capacity(spans.len());
    let mut top = 0.0;
    let mut start = 0;
    while start < spans.len() {
        // The row takes cards while their columns fit; the first always does.
        let mut end = start;
        let mut filled = 0;
        while end < spans.len() && filled + spans[end] <= columns {
            filled += spans[end];
            end += 1;
        }
        let room = width - gap * (end - start - 1) as f32;
        let height = heights[start..end].iter().copied().fold(0.0, f32::max);
        let mut x = 0.0;
        for &span in &spans[start..end] {
            let card_width = room * f32::from(span) / f32::from(filled);
            places.push(Place {
                x,
                y: top,
                width: card_width,
                height,
            });
            x += card_width + gap;
        }
        top += height + gap;
        start = end;
    }
    (places, (top - gap).max(0.0))
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
