//! Settings → Shortcuts (feature-inventory §1.4): a table of the rebindable
//! bindings — click a combo to record (Esc cancels), live conflict detection,
//! per-row Reset and Restore defaults. Changes emit [`ShortcutsEvent`]; the
//! shell persists them and re-applies the app keymap.
//!
//! Search by keys (as in ChatGPT's and Codex's desktop apps): press a
//! combination and the page says what in Keron uses it, read from the live
//! keymap ([`key_uses`]).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, KeyBinding, KeyBindingContextPredicate, Keystroke,
    SharedString, Window, div, prelude::*, px,
};

use crate::appshots::{AppshotCapabilities, AppshotDestination};
use crate::popover::{self, ScrollRailHost};

#[path = "appshots.rs"]
mod appshots_page;
use crate::settings::widgets;
use crate::settings::{
    ComposerSendBehavior, KeymapConfig, ShortcutId, combo_from_keystroke, display_combo,
};
use crate::state::AppState;
use crate::theme::Theme;

/// Outcome of one keystroke while recording. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Esc — abandon recording, keep the old combo.
    Cancelled,
    /// A bare modifier (or unusable key) — stay recording.
    Ignored,
    /// A full combo landed.
    Set(String),
}

pub fn record_key(key: &str, ctrl: bool, alt: bool, shift: bool, cmd: bool) -> RecordOutcome {
    if key.eq_ignore_ascii_case("escape") {
        return RecordOutcome::Cancelled;
    }
    match combo_from_keystroke(ctrl, alt, shift, cmd, key) {
        Some(combo) => RecordOutcome::Set(combo),
        None => RecordOutcome::Ignored,
    }
}

#[derive(Debug, Clone)]
pub enum ShortcutsEvent {
    /// The keymap changed — persist + re-apply.
    KeymapChanged(KeymapConfig),
    /// The Escape fallback changed — persist it locally.
    EscapeStopsActiveAgentChanged(bool),
    /// The composer send behavior changed — persist + re-apply.
    ComposerSendBehaviorChanged(ComposerSendBehavior),
    AppshotsChanged {
        enabled: bool,
        sound_enabled: bool,
        destination: AppshotDestination,
    },
}

pub struct ShortcutsPage {
    appshots_page: bool,
    general_page: bool,
    appshots_focus_pending: bool,
    scroll: crate::settings::widgets::PageScroll,
    /// Working copy (kept in sync with the shell via change events).
    keymap: KeymapConfig,
    escape_stops_active_agent: bool,
    composer_send_behavior: ComposerSendBehavior,
    recording: Option<ShortcutId>,
    recording_blur: Option<gpui::Subscription>,
    recording_interceptor: Option<gpui::Subscription>,
    /// Search by keys: `Some` while it listens.
    key_search: Option<KeySearch>,
    key_search_subscriptions: Option<[gpui::Subscription; 3]>,
    /// The Search by keys button's own handle, a tab stop while no search
    /// runs. During one the button stops tracking it, so pressing the button
    /// in any way leaves focus on the page.
    key_search_button: FocusHandle,
    /// Where a search's answer still has to scroll into view.
    reveal: Rc<Cell<Reveal>>,
    /// A rejected record attempt ("{Combo} is already assigned to {label}.") —
    /// conflicts never persist; they're refused at record time, as in zeron.
    conflict_notice: Option<SharedString>,
    focus: FocusHandle,
    appshots_enabled: bool,
    appshot_sound_enabled: bool,
    appshot_destination: AppshotDestination,
    appshot_capabilities: AppshotCapabilities,
    send_select: widgets::SelectState,
    destination_select: widgets::SelectState,
    capture_access_prompted: bool,
    semantic_access_prompted: bool,
    /// Settings → General's thread naming card (its own title-bound picker).
    thread_naming: Entity<crate::settings::thread_naming::ThreadNamingCard>,
    _state: Entity<AppState>,
}

impl EventEmitter<ShortcutsEvent> for ShortcutsPage {}

impl ShortcutsPage {
    pub fn new(
        state: Entity<AppState>,
        keymap: KeymapConfig,
        escape_stops_active_agent: bool,
        composer_send_behavior: ComposerSendBehavior,
        appshots_enabled: bool,
        appshot_sound_enabled: bool,
        appshot_destination: AppshotDestination,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|_, _| crate::appshots::set_recording(false))
            .detach();
        Self {
            appshots_page: false,
            general_page: false,
            appshots_focus_pending: false,
            scroll: crate::settings::widgets::PageScroll::default(),
            keymap,
            escape_stops_active_agent,
            composer_send_behavior,
            recording: None,
            recording_blur: None,
            recording_interceptor: None,
            key_search: None,
            key_search_subscriptions: None,
            key_search_button: cx.focus_handle().tab_index(0).tab_stop(true),
            reveal: Rc::default(),
            conflict_notice: None,
            focus: cx.focus_handle(),
            appshots_enabled,
            appshot_sound_enabled,
            appshot_destination,
            appshot_capabilities: crate::appshots::capabilities(),
            send_select: widgets::SelectState::default(),
            destination_select: widgets::SelectState::default(),
            capture_access_prompted: false,
            semantic_access_prompted: false,
            thread_naming: {
                let state = state.clone();
                cx.new(|cx| crate::settings::thread_naming::ThreadNamingCard::new(state, cx))
            },
            _state: state,
        }
    }

    pub fn show_appshots(&mut self, appshots: bool) {
        self.show_section(appshots, false);
    }

    pub fn show_section(&mut self, appshots: bool, general: bool) {
        if self.appshots_page != appshots || self.general_page != general {
            self.stop_recording();
            self.stop_key_search();
            self.conflict_notice = None;
            self.appshots_page = appshots;
            self.general_page = general;
            self.appshots_focus_pending = appshots;
            // One scroll state serves these pages — rewind it so each opens
            // at the top instead of where the other was left.
            self.scroll.reset();
            if appshots {
                self.appshot_capabilities = crate::appshots::capabilities();
            }
        }
    }

    fn start_recording(&mut self, id: ShortcutId, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_key_search();
        self.recording = Some(id);
        crate::appshots::set_recording(true);
        let page = cx.entity().downgrade();
        // Bound actions run before Div key listeners. Intercept first so a
        // conflicting chord is recorded/refused instead of running its action.
        self.recording_interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let _ = page.update(cx, |page, cx| {
                if page.focus.is_focused(window) {
                    page.record_keystroke(&event.keystroke, cx);
                }
            });
        }));
        self.recording_blur = Some(cx.on_blur(&self.focus, window, |this, _, cx| {
            this.stop_recording();
            cx.notify();
        }));
        self.conflict_notice = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self) {
        // Only a recording releases Appshots' hotkey: a key search may hold it.
        if self.recording.take().is_some() {
            crate::appshots::set_recording(false);
        }
        self.recording_blur = None;
        self.recording_interceptor = None;
    }

    /// Listen for a key combination and show what uses it. Like recording,
    /// it intercepts before bound actions run, and holds Appshots' hotkey so
    /// that combination can be searched for too. Unlike recording it keeps
    /// listening, so it ends when focus leaves the page or the owner leaves
    /// the window: the hold must not follow them into other apps.
    fn start_key_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_recording();
        self.conflict_notice = None;
        self.key_search = Some(KeySearch::default());
        self.reveal.set(Reveal::Results { row: false });
        crate::appshots::set_recording(true);
        let page = cx.entity().downgrade();
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            let _ = page.update(cx, |page, cx| {
                if page.focus.is_focused(window) {
                    page.search_keystroke(&event.keystroke, cx);
                }
            });
        });
        let blur = cx.on_blur(&self.focus, window, |this, _, cx| {
            this.stop_key_search();
            cx.notify();
        });
        // gpui reports leaving the window as a blur too, but only on its next
        // frame, which a hidden or covered window may not draw.
        let deactivate = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.stop_key_search();
                cx.notify();
            }
        });
        self.key_search_subscriptions = Some([interceptor, blur, deactivate]);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn stop_key_search(&mut self) {
        if self.key_search.take().is_some() {
            crate::appshots::set_recording(false);
        }
        self.key_search_subscriptions = None;
        self.reveal.set(Reveal::Done);
    }

    fn toggle_key_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.key_search.is_some() {
            self.stop_key_search();
            cx.notify();
        } else {
            self.start_key_search(window, cx);
        }
    }

    fn search_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let mods = &keystroke.modifiers;
        match record_key(
            &keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            RecordOutcome::Cancelled => self.stop_key_search(),
            RecordOutcome::Ignored => {}
            RecordOutcome::Set(combo) => {
                let uses = {
                    let keymap = cx.key_bindings();
                    let keymap = keymap.borrow();
                    key_uses(keymap.bindings(), &self.keymap, keystroke)
                };
                self.reveal.set(Reveal::Results {
                    row: uses
                        .iter()
                        .any(|found| found.shortcut.is_some_and(listed_here)),
                });
                self.key_search = Some(KeySearch {
                    combo: Some(combo),
                    uses,
                });
            }
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        cx.emit(ShortcutsEvent::KeymapChanged(self.keymap.clone()));
        cx.notify();
    }

    fn set_escape_stops_active_agent(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.escape_stops_active_agent != enabled {
            self.escape_stops_active_agent = enabled;
            cx.emit(ShortcutsEvent::EscapeStopsActiveAgentChanged(enabled));
            cx.notify();
        }
    }

    fn set_composer_send_behavior(
        &mut self,
        behavior: ComposerSendBehavior,
        cx: &mut Context<Self>,
    ) {
        if self.composer_send_behavior != behavior {
            self.composer_send_behavior = behavior;
            self.conflict_notice = None;
            cx.emit(ShortcutsEvent::ComposerSendBehaviorChanged(behavior));
            cx.notify();
        }
    }

    fn commit_appshots(&self, cx: &mut Context<Self>) {
        cx.emit(ShortcutsEvent::AppshotsChanged {
            enabled: self.appshots_enabled,
            sound_enabled: self.appshot_sound_enabled,
            destination: self.appshot_destination,
        });
    }

    fn record_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(recording) = self.recording else {
            return;
        };
        let mods = &keystroke.modifiers;
        match record_key(
            &keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            RecordOutcome::Cancelled => {
                self.stop_recording();
                cx.notify();
            }
            RecordOutcome::Ignored => {}
            RecordOutcome::Set(combo) => {
                self.stop_recording();
                self.conflict_notice =
                    refusal(&self.keymap, self.composer_send_behavior, recording, &combo);
                if self.conflict_notice.is_none() {
                    self.keymap.set(recording, combo);
                    self.commit(cx);
                }
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    /// One shortcut row: label left, Reset when customized, and
    /// the click-to-record combo chip (recording inverts it to
    /// white-on-black). `ix` is the id's position in [`ShortcutId::ALL`]
    /// (unique element ids across the group cards); `gx` is the row's place
    /// in its own card (separator rule). A key search hit is tinted, and
    /// `reveal` marks the row that scrolls into view.
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        id: ShortcutId,
        ix: usize,
        gx: usize,
        recording: Option<ShortcutId>,
        hit: bool,
        reveal: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        widgets::card_row(theme, gx == 0)
            .relative()
            .when(reveal, |row| {
                row.child(reveal_marker(
                    self.reveal.clone(),
                    self.scroll.scroll.clone(),
                    false,
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .child(
                        widgets::row_title(theme, id.label())
                            .when(hit, |title| title.text_color(theme.accent)),
                    )
                    .when(
                        matches!(id, ShortcutId::NextSession | ShortcutId::PrevSession),
                        |row| {
                            row.child(widgets::meta_line(
                                theme,
                                vec![
                                    div()
                                        .child("Navigate within the focused pane.")
                                        .into_any_element(),
                                ],
                            ))
                        },
                    ),
            )
            .child(self.render_binding_control(id, ix, recording, hit, theme, cx))
    }

    fn render_binding_control(
        &self,
        id: ShortcutId,
        ix: usize,
        recording: Option<ShortcutId>,
        hit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let page = cx.entity().downgrade();
        let reset_page = page.clone();
        binding_control(
            id,
            ix,
            self.keymap.get(id),
            recording == Some(id),
            hit,
            theme,
            move |_, cx| {
                reset_page
                    .update(cx, |this, cx| {
                        this.keymap.reset(id);
                        this.stop_recording();
                        this.commit(cx);
                    })
                    .ok();
            },
            move |window, cx| {
                page.update(cx, |this, cx| this.start_recording(id, window, cx))
                    .ok();
            },
        )
    }

    /// Search by keys: starts listening, and stops it when pressed again.
    /// Listening wears the recording chip's accent wash. While it listens
    /// the button takes no focus, so a right-click or a press dragged off it
    /// cannot pull focus from the page and leave a search that hears nothing.
    fn render_key_search_button(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let accent = theme.accent;
        let active = self.key_search.is_some();
        widgets::ghost_action(theme)
            .id("shortcuts-key-search")
            .debug_selector(|| "shortcuts-key-search".into())
            .when(!active, |el| el.track_focus(&self.key_search_button))
            .role(gpui::Role::Button)
            .aria_label("Search by keys")
            .aria_toggled(if active {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .flex_none()
            .border_1()
            .border_color(gpui::transparent_black())
            .focus_visible(move |style| style.border_color(accent))
            .when(active, |el| {
                el.bg(accent.opacity(0.16))
                    .border_color(accent.opacity(0.55))
                    .text_color(theme.text)
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_key_search(window, cx)))
            .child(
                crate::icons::icon(crate::icons::KEYBOARD)
                    .size(px(14.0))
                    .text_color(if active { theme.text } else { theme.text_muted }),
            )
            .child(SharedString::from("Search by keys"))
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    // Kept for the appshots half of this page (appshots.rs), whose host still
    // carries the drag-move listener itself.
    fn on_bar_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<popover::MenuScrollbarDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rail_drag_to(event.event.position.y) {
            cx.notify();
        }
    }

    /// The shared rail under the id of whichever page is showing. Kept as a
    /// method because the appshots half of this page renders through it.
    fn render_scrollbar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let id = if self.appshots_page {
            "appshots-settings-page-scrollbar"
        } else if self.general_page {
            "general-settings-page-scrollbar"
        } else {
            "shortcuts-page-scrollbar"
        };
        popover::rail(self, id, theme, cx)
    }
}

impl popover::ScrollRailHost for ShortcutsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

/// One rebindable shortcut on the settings page of the feature it drives
/// (Settings → Voice). Records and refuses combos exactly like a Shortcuts
/// row, then saves straight to the store and re-applies the keymap.
pub struct ShortcutField {
    id: ShortcutId,
    focus: FocusHandle,
    recording: bool,
    recording_subscriptions: Option<[gpui::Subscription; 3]>,
    notice: Option<SharedString>,
}

impl ShortcutField {
    pub fn new(id: ShortcutId, cx: &mut Context<Self>) -> Self {
        cx.on_release(|field, _| {
            if field.recording {
                crate::appshots::set_recording(false);
            }
        })
        .detach();
        Self {
            id,
            focus: cx.focus_handle(),
            recording: false,
            recording_subscriptions: None,
            notice: None,
        }
    }

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = true;
        crate::appshots::set_recording(true);
        let field = cx.entity().downgrade();
        // Bound actions run before Div key listeners; intercept first so the
        // chord being recorded (⌘D itself) never starts dictation.
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            let _ = field.update(cx, |field, cx| {
                if field.focus.is_focused(window) {
                    field.record_keystroke(&event.keystroke, cx);
                }
            });
        });
        let blur = cx.on_blur(&self.focus, window, |field, _, cx| {
            field.stop_recording();
            cx.notify();
        });
        // The Voice page outlives its window (closing it keeps the app
        // running), and a close does not blur. Never leave Appshots'
        // capture shortcut suspended.
        let field = cx.entity().downgrade();
        let closed = cx.on_window_closed(move |cx, _| {
            field
                .update(cx, |field, cx| {
                    field.stop_recording();
                    cx.notify();
                })
                .ok();
        });
        self.recording_subscriptions = Some([interceptor, blur, closed]);
        self.notice = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self) {
        if self.recording {
            crate::appshots::set_recording(false);
        }
        self.recording = false;
        self.recording_subscriptions = None;
    }

    fn record_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let mods = &keystroke.modifiers;
        match record_key(
            &keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            RecordOutcome::Cancelled => self.stop_recording(),
            RecordOutcome::Ignored => {}
            RecordOutcome::Set(combo) => {
                self.stop_recording();
                let current = crate::settings::current(cx);
                self.notice = refusal(
                    &current.keymap,
                    current.composer_send_behavior,
                    self.id,
                    &combo,
                );
                if self.notice.is_none() {
                    self.save(Some(combo), cx);
                }
            }
        }
        cx.notify();
        cx.stop_propagation();
    }

    /// `None` restores the default binding.
    fn save(&mut self, combo: Option<String>, cx: &mut Context<Self>) {
        let id = self.id;
        crate::settings::update(
            crate::settings::SavePolicy::Immediate,
            cx,
            |settings| match combo {
                Some(combo) => settings.keymap.set(id, combo),
                None => settings.keymap.reset(id),
            },
        );
        let current = crate::settings::current(cx);
        crate::shell::apply_keymap(cx, &current.keymap, current.composer_send_behavior);
        cx.notify();
    }
}

impl Render for ShortcutField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let combo = crate::settings::current(cx).keymap.get(self.id).to_string();
        let ix = ShortcutId::ALL
            .iter()
            .position(|&id| id == self.id)
            .unwrap_or(0);
        let reset = cx.entity().downgrade();
        let record = reset.clone();
        let helper: Option<SharedString> = if self.recording {
            Some("Press Escape to cancel.".into())
        } else {
            self.notice.clone()
        };
        div()
            .id("shortcut-field")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .items_end()
            .gap(px(4.0))
            .child(binding_control(
                self.id,
                ix,
                &combo,
                self.recording,
                false,
                &theme,
                move |_, cx| {
                    reset
                        .update(cx, |field, cx| {
                            field.stop_recording();
                            field.notice = None;
                            field.save(None, cx);
                        })
                        .ok();
                },
                move |window, cx| {
                    record
                        .update(cx, |field, cx| field.start_recording(window, cx))
                        .ok();
                },
            ))
            .children(helper.map(|helper| {
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(helper)
            }))
    }
}

/// Reset (when customized) plus the click-to-record combo chip; recording
/// inverts it to the accent wash, and a key search hit wears the same wash
/// around its combo. Shared by Settings → Shortcuts and the feature pages
/// that own a shortcut ([`ShortcutField`]).
#[allow(clippy::too_many_arguments)]
fn binding_control(
    id: ShortcutId,
    ix: usize,
    combo: &str,
    is_recording: bool,
    is_hit: bool,
    theme: &Theme,
    on_reset: impl Fn(&mut Window, &mut gpui::App) + 'static,
    on_record: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> gpui::Div {
    let accent = theme.accent;
    let combo = combo.to_string();
    let non_default = combo != id.default_combo();
    let chip_text: SharedString = if is_recording {
        "Press keys…".into()
    } else {
        display_combo(&combo).into()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(20.0))
        .when(non_default && !is_recording, |el| {
            el.child(
                div()
                    .id(("shortcut-reset", ix))
                    .role(gpui::Role::Button)
                    .aria_label(format!("Reset {} shortcut", id.label()))
                    .min_h(px(24.0))
                    .tab_index(0)
                    .focus_visible(move |style| style.border_2().border_color(accent))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .on_click(move |_, window, cx| on_reset(window, cx))
                    .child(SharedString::from("Reset")),
            )
        })
        .child(
            div()
                .id(("shortcut-combo", ix))
                .role(gpui::Role::Button)
                .aria_label(format!(
                    "Change {} shortcut: {}",
                    id.label(),
                    display_combo(&combo)
                ))
                .tab_index(0)
                .min_w(px(96.0))
                .h(px(widgets::SELECT_HEIGHT))
                .px(px(12.0))
                .rounded(px(8.0))
                // Same glass wash as the settings dropdowns; the
                // transparent edge is held for the focus ring.
                .border_1()
                .border_color(gpui::transparent_black())
                .focus_visible(move |style| style.border_color(accent))
                .flex()
                .items_center()
                .justify_center()
                .font_family(theme.font_mono.clone())
                .text_size(crate::typography::ui_rems(12.0))
                .cursor_pointer()
                .map(|el| {
                    if is_recording || is_hit {
                        el.bg(accent.opacity(0.16))
                            .border_color(accent.opacity(0.55))
                            .text_color(theme.text)
                    } else {
                        let hover_key = format!("shortcut-combo-{ix}-hover");
                        el.bg(crate::motion::hover_blend(
                            &hover_key,
                            widgets::select_fill(theme, false),
                            widgets::select_fill(theme, true),
                        ))
                        .on_hover(crate::motion::hover_listener(hover_key))
                        .text_color(theme.text)
                    }
                })
                .on_click(move |_, window, cx| on_record(window, cx))
                .child(chip_text),
        )
}

/// Why `combo` cannot be bound to `id`, if it cannot. Conflicts never
/// persist; they're refused at record time, naming the owner (zeron
/// settings.shortcuts.tsx: "… is already assigned to …"). Pure.
fn refusal(
    keymap: &KeymapConfig,
    behavior: ComposerSendBehavior,
    id: ShortcutId,
    combo: &str,
) -> Option<SharedString> {
    if id == ShortcutId::CaptureAppshot && crate::appshots::validate_shortcut(combo).is_err() {
        return Some(
            format!(
                "Use {} with a letter, number, function key or navigation key.",
                if cfg!(target_os = "macos") {
                    "Control, Option or Command"
                } else {
                    "Control or Alt"
                }
            )
            .into(),
        );
    }
    if send_combo_is_reserved(behavior, combo) {
        return Some(format!("{} is reserved for the composer.", display_combo(combo)).into());
    }
    conflict_owner(keymap, id, combo).map(|owner| {
        // Name the page for shortcuts that live on their feature's page.
        let page = match group(owner) {
            page @ ("Appshots" | "Voice") => format!(" in Settings → {page}"),
            _ => String::new(),
        };
        format!(
            "{} is already assigned to {}{page}.",
            display_combo(combo),
            owner.label()
        )
        .into()
    })
}

/// The shortcut (other than `id`) already bound to `combo`, if any. Pure.
pub fn conflict_owner(keymap: &KeymapConfig, id: ShortcutId, combo: &str) -> Option<ShortcutId> {
    ShortcutId::ALL
        .into_iter()
        .find(|&other| other.available() && other != id && keymap.get(other) == combo)
}

/// One search by keys: the last combination pressed (`None` until one
/// lands) and what Keron uses it for.
#[derive(Debug, Default)]
struct KeySearch {
    combo: Option<String>,
    uses: Vec<KeyUse>,
}

/// One thing in Keron a key combination does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyUse {
    /// The rebindable shortcut it is, if any.
    pub shortcut: Option<ShortcutId>,
    /// What it does: the shortcut's label, or words derived from the action.
    pub name: String,
    /// Where it works ("Composer", "Browser"). Empty: everywhere.
    pub places: Vec<String>,
}

/// What `keystroke` does in Keron, read from the live keymap: the bindings
/// gpui dispatches, which `shell::apply_keymap` builds from the owner's
/// rebinds, so a binding added anywhere shows up here by itself. The Appshot
/// hotkey is the one shortcut the OS delivers outside that keymap, so it is
/// matched against `keymap`. Rebindable shortcuts come first, in table
/// order; the rest follow by precedence, one entry per name.
pub fn key_uses<'a>(
    bindings: impl DoubleEndedIterator<Item = &'a KeyBinding>,
    keymap: &KeymapConfig,
    keystroke: &Keystroke,
) -> Vec<KeyUse> {
    let actions: Vec<_> = ShortcutId::ALL
        .into_iter()
        .filter_map(|id| Some((id, crate::shell::shortcut_action(id)?)))
        .collect();
    let mut shortcuts: Vec<ShortcutId> = Vec::new();
    let appshot = ShortcutId::CaptureAppshot;
    if appshot.available()
        && Keystroke::parse(&crate::settings::platform_combo(keymap.get(appshot)))
            .is_ok_and(|combo| combo.modifiers == keystroke.modifiers && combo.key == keystroke.key)
    {
        shortcuts.push(appshot);
    }
    // `None` places: some binding of that name works everywhere.
    let mut others: Vec<(String, Option<Vec<String>>)> = Vec::new();
    for binding in bindings.rev() {
        let action = binding.action();
        if gpui::is_no_action(action)
            || gpui::is_unbind(action)
            || binding.match_keystrokes(std::slice::from_ref(keystroke)) != Some(false)
        {
            continue;
        }
        if let Some(&(id, _)) = actions.iter().find(|(_, bound)| bound.partial_eq(action)) {
            if !shortcuts.contains(&id) {
                shortcuts.push(id);
            }
            continue;
        }
        let name = words(action.name().rsplit("::").next().unwrap_or_default());
        let place = binding.predicate().map(|predicate| place_name(&predicate));
        match others
            .iter_mut()
            .find(|(other, _)| other.eq_ignore_ascii_case(&name))
        {
            Some((_, places)) => match (places, place) {
                (Some(places), Some(place)) if !places.contains(&place) => places.push(place),
                (places, None) => *places = None,
                _ => {}
            },
            None => others.push((name, place.map(|place| vec![place]))),
        }
    }
    shortcuts.sort_by_key(|id| ShortcutId::ALL.iter().position(|other| other == id));
    shortcuts
        .into_iter()
        .map(|id| KeyUse {
            shortcut: Some(id),
            name: id.label().to_string(),
            places: Vec::new(),
        })
        .chain(others.into_iter().map(|(name, places)| KeyUse {
            shortcut: None,
            name,
            places: places.unwrap_or_default(),
        }))
        .collect()
}

/// Where a contextual binding works, named for the owner. Contexts with no
/// name here read as their own words ("ColorPicker" → "Color picker").
fn place_name(predicate: &KeyBindingContextPredicate) -> String {
    fn context(predicate: &KeyBindingContextPredicate) -> Option<&str> {
        use KeyBindingContextPredicate as P;
        match predicate {
            P::Identifier(name) => Some(name),
            P::Descendant(_, child) => context(child),
            P::And(left, right) | P::Or(left, right) => context(left).or_else(|| context(right)),
            P::Equal(..) | P::NotEqual(..) | P::Not(_) => None,
        }
    }
    match context(predicate) {
        Some("MessageComposer") => "Composer".into(),
        Some("Composer") => "Text fields".into(),
        Some("PaletteSearch") => "Search fields".into(),
        Some("Input") => "File editor".into(),
        Some(other) => words(other),
        None => predicate.to_string(),
    }
}

/// An identifier as plain words: "SelectAll" → "Select all", "OpenURLBar" →
/// "Open URL bar".
fn words(name: &str) -> String {
    let chars: Vec<char> = name.chars().filter(|c| *c != '_').collect();
    let mut words: Vec<String> = Vec::new();
    for (ix, &c) in chars.iter().enumerate() {
        let starts_word = ix == 0
            || (c.is_uppercase()
                && (!chars[ix - 1].is_uppercase()
                    || chars.get(ix + 1).is_some_and(|next| next.is_lowercase())));
        if starts_word {
            words.push(String::new());
        }
        if let Some(word) = words.last_mut() {
            word.push(c);
        }
    }
    words
        .iter()
        .enumerate()
        .map(|(ix, word)| {
            let acronym = word.chars().count() > 1 && !word.chars().any(char::is_lowercase);
            if ix == 0 || acronym {
                word.clone()
            } else {
                word.to_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `id` has a row in this page's table (Voice and Appshots keep
/// theirs on their own pages).
fn listed_here(id: ShortcutId) -> bool {
    !matches!(group(id), "Appshots" | "Voice")
}

/// Where a key search's answer still has to scroll into view. The results
/// card sits above the table, so its marker runs first in each frame; when a
/// hit row follows, it hands the row the shift it chose.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Reveal {
    #[default]
    Done,
    /// Show the results card, then the first hit row if `row`.
    Results { row: bool },
    /// The results card asked for `shift`; the row adds what it needs.
    Row { shift: gpui::Pixels },
}

/// Paints nothing. After a key search, scrolls the page just enough to show
/// the results card (`results`) or the first hit row clear of the titlebar
/// and the bottom fade: the card first, then the row as far as both fit, so
/// the answer is on screen even when no row matches.
fn reveal_marker(
    reveal: Rc<Cell<Reveal>>,
    scroll: gpui::ScrollHandle,
    results: bool,
) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            let (shift, then_row) = match (results, reveal.get()) {
                (true, Reveal::Results { row }) => (px(0.0), row),
                (false, Reveal::Row { shift }) => (shift, false),
                _ => return,
            };
            let view = scroll.bounds();
            let top = view.top() + px(Theme::TITLEBAR_HEIGHT + 16.0);
            let bottom = view.bottom() - px(16.0);
            let (target_top, target_bottom) = (bounds.top() + shift, bounds.bottom() + shift);
            let more = if target_top < top {
                top - target_top
            } else if target_bottom > bottom {
                (bottom - target_bottom).max(top - target_top)
            } else {
                px(0.0)
            };
            let offset = scroll.offset();
            let y = (offset.y + shift + more).clamp(-scroll.max_offset().y, px(0.0));
            if then_row {
                reveal.set(Reveal::Row {
                    shift: y - offset.y,
                });
                return;
            }
            reveal.set(Reveal::Done);
            if y != offset.y {
                scroll.set_offset(gpui::point(offset.x, y));
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

pub fn send_combo_is_reserved(_behavior: ComposerSendBehavior, combo: &str) -> bool {
    combo == "mod-enter"
}

pub fn modifier_send_label(is_macos: bool) -> &'static str {
    if is_macos { "⌘ Enter" } else { "Ctrl Enter" }
}

/// The page's sections, in display order. [`group`] is a total match, so every
/// [`ShortcutId::ALL`] entry lands in exactly one — a shortcut added later
/// extends the match and appears on the page by construction
/// (`every_shortcut_lands_in_a_rendered_group` holds the other half: its group
/// name must be listed here).
const GROUP_ORDER: [&str; 9] = [
    "Appearance",
    "Files",
    "Browser",
    "Panels",
    "Sessions",
    "Projects",
    "Jump to session",
    "Appshots",
    "Voice",
];

/// The section a shortcut's row renders under.
fn group(id: ShortcutId) -> &'static str {
    match id {
        ShortcutId::RandomWallpaper => "Appearance",
        ShortcutId::CaptureAppshot => "Appshots",
        ShortcutId::SaveFile => "Files",
        ShortcutId::BrowserReload => "Browser",
        ShortcutId::ToggleSidebar
        | ShortcutId::ToggleChanges
        | ShortcutId::ToggleFiles
        | ShortcutId::ToggleTerminal => "Panels",
        ShortcutId::NewProject => "Projects",
        ShortcutId::ToggleDictation => "Voice",
        ShortcutId::OpenModelPicker
        | ShortcutId::NewSession
        | ShortcutId::ShowHome
        | ShortcutId::NextSession
        | ShortcutId::PrevSession
        | ShortcutId::ArchiveSession => "Sessions",
        ShortcutId::JumpSession(_) => "Jump to session",
    }
}

/// What a key search found, above the table: the combination pressed and
/// each use of it, or a prompt while nothing has been pressed yet.
fn render_key_search(search: &KeySearch, theme: &Theme) -> gpui::Div {
    let row = |gx: usize, title: SharedString, meta: Vec<String>| {
        widgets::card_row(theme, gx == 0).child(
            div()
                .flex_1()
                .min_w_0()
                .child(widgets::row_title(theme, title))
                .when(!meta.is_empty(), |el| {
                    el.child(widgets::meta_line(
                        theme,
                        meta.into_iter()
                            .map(|fragment| div().child(fragment).into_any_element())
                            .collect(),
                    ))
                }),
        )
    };
    let mut card = widgets::section_card(theme).mt_0();
    let label = match &search.combo {
        None => {
            card = card.child(row(0, "Press a key combination.".into(), Vec::new()));
            "Search by keys".to_string()
        }
        Some(combo) => {
            if search.uses.is_empty() {
                card = card.child(row(0, "Not used in Keron".into(), Vec::new()));
            }
            for (gx, found) in search.uses.iter().enumerate() {
                let meta = match found.shortcut {
                    Some(id) if listed_here(id) => vec!["Highlighted below".to_string()],
                    Some(id) => vec![format!("Settings → {}", group(id))],
                    None if found.places.is_empty() => vec!["Everywhere".to_string()],
                    None => found.places.clone(),
                };
                card = card.child(row(gx, found.name.clone().into(), meta));
            }
            display_combo(combo)
        }
    };
    widgets::section(theme, label, card)
}

impl Render for ShortcutsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Feature pages (Voice) rebind their shortcut through the store. Read
        // it back so a later commit here never republishes an older keymap.
        if self.recording.is_none()
            && let Some(store) = cx.try_global::<crate::settings::SettingsStore>()
        {
            self.keymap.clone_from(&store.current.keymap);
        }
        if self.appshots_page {
            if std::mem::take(&mut self.appshots_focus_pending) {
                window.focus(&self.focus, cx);
            }
            return self.render_appshots(window, cx);
        }
        let theme = Theme::of(cx).for_settings_surface();
        let recording = self.recording;
        let escape_stops_active_agent = self.escape_stops_active_agent;
        let send_behavior = self.composer_send_behavior;
        let compact_mode = crate::settings::transcript_compact_mode(cx);
        let customized = self.keymap != KeymapConfig::default()
            || escape_stops_active_agent
            || send_behavior != ComposerSendBehavior::default();
        let modifier_label = modifier_send_label(cfg!(target_os = "macos"));

        let send_behaviors = [
            (ComposerSendBehavior::Enter, "Enter"),
            (ComposerSendBehavior::ModEnter, modifier_label),
        ];
        let send_behavior_control = widgets::select(
            "composer-send-behavior",
            "Send messages with",
            &theme,
            |page: &mut Self| &mut page.send_select,
        )
        .options(
            send_behaviors
                .iter()
                .map(|(_, label)| widgets::SelectOption::new(*label)),
            send_behaviors
                .iter()
                .position(|(behavior, _)| *behavior == send_behavior)
                .unwrap_or_default(),
        )
        .width(128.0)
        .font_family(theme.font_mono.clone())
        .on_select(move |page, ix, _, cx| page.set_composer_send_behavior(send_behaviors[ix].0, cx))
        .render(&self.send_select, cx);

        let send_behavior_row = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Send messages with")),
            )
            .child(send_behavior_control);
        let compact_mode_row = widgets::card_row(&theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(&theme, "Compact mode"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child("Collapse thinking and tools.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::toggle_switch(&theme, compact_mode, "transcript-compact-mode")
                    .id("transcript-compact-mode-toggle")
                    .tab_index(0)
                    .role(gpui::Role::Switch)
                    .aria_label("Compact mode")
                    .aria_toggled(if compact_mode {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::settings::set_transcript_compact_mode(!compact_mode, cx);
                        cx.notify();
                    })),
            );
        let compact_model_picker = crate::settings::compact_model_picker(cx);
        let compact_model_picker_row = widgets::card_row(&theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(&theme, "Compact model picker"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child("Adjust effort with a slider, then open the model list when needed.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::toggle_switch(&theme, compact_model_picker, "compact-model-picker")
                    .id("compact-model-picker-toggle")
                    .tab_index(0)
                    .role(gpui::Role::Switch)
                    .aria_label("Compact model picker")
                    .aria_toggled(if compact_model_picker {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::settings::update(
                            crate::settings::SavePolicy::Debounced,
                            cx,
                            |settings| settings.compact_model_picker = !compact_model_picker,
                        );
                        cx.refresh_windows();
                        cx.notify();
                    })),
            );
        let escape_behavior_row = widgets::card_row(&theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Stop agent with Escape"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![
                            div()
                                .child("When no dialog or menu is open.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(
                widgets::toggle_switch(
                    &theme,
                    escape_stops_active_agent,
                    "escape-stops-active-agent",
                )
                .id("escape-stops-active-agent-toggle")
                .debug_selector(|| "escape-stops-active-agent-toggle".into())
                .tab_index(0)
                .role(gpui::Role::Switch)
                .aria_label("Stop active agent with Escape")
                .aria_toggled(if escape_stops_active_agent {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_escape_stops_active_agent(!escape_stops_active_agent, cx);
                })),
            );
        if self.general_page {
            let scrollbar = self.render_scrollbar(&theme, cx);
            return div()
                .id("general-settings-page-host")
                .relative()
                .size_full()
                .on_hover(cx.listener(Self::on_scroll_hovered))
                .child(
                    crate::edge_fade::edge_faded(
                        16.0,
                        true,
                        true,
                        div()
                            .id("general-settings-page")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll.scroll)
                            .child(
                                widgets::page_column()
                                    .child(widgets::page_header(&theme, "General", None))
                                    .child(
                                        widgets::section_card(&theme)
                                            .child(send_behavior_row)
                                            .child(compact_mode_row)
                                            .child(compact_model_picker_row)
                                            .child(escape_behavior_row),
                                    )
                                    .child(self.thread_naming.clone()),
                            ),
                    )
                    .fade_overflow_y(&self.scroll.scroll),
                )
                .children(scrollbar)
                .into_any_element();
        }
        // A key search tints its rows; the first one in page order scrolls
        // into view.
        let hits: Vec<ShortcutId> = self
            .key_search
            .iter()
            .flat_map(|search| search.uses.iter().filter_map(|found| found.shortcut))
            .collect();
        let mut reveal_pending = true;
        // One block per group, each under its small section label — the flat
        // 16-row table read as one undifferentiated wall. `ix` (the id's
        // position in ALL) keys the interactive elements, so ids stay unique
        // across blocks. The section wrapper owns the spacing, so the block's
        // own top margin is zeroed.
        let mut groups: Vec<gpui::AnyElement> = Vec::new();
        for name in GROUP_ORDER {
            // Feature pages own these: Settings → Appshots and → Voice.
            if name == "Appshots" || name == "Voice" {
                continue;
            }
            let mut card = widgets::section_card(&theme).mt_0();
            let ids = ShortcutId::ALL.into_iter().filter(|&id| group(id) == name);
            for (gx, id) in ids.enumerate() {
                let ix = ShortcutId::ALL.iter().position(|&a| a == id).unwrap_or(0);
                let hit = hits.contains(&id);
                let reveal = hit && std::mem::take(&mut reveal_pending);
                card = card.child(self.render_row(id, ix, gx, recording, hit, reveal, &theme, cx));
            }
            groups.push(widgets::section(&theme, name, card).into_any_element());
        }
        let key_search = self.key_search.as_ref().map(|search| {
            render_key_search(search, &theme)
                .relative()
                .debug_selector(|| "shortcuts-key-search-results".into())
                .child(reveal_marker(
                    self.reveal.clone(),
                    self.scroll.scroll.clone(),
                    true,
                ))
        });

        // Helper line stays in the muted tone even for a rejected conflict —
        // the message names the specific clash (zeron settings.shortcuts.tsx).
        let helper: SharedString = if recording.is_some() {
            "Press Escape to cancel.".into()
        } else if self.key_search.is_some() {
            "Press Escape to stop searching.".into()
        } else if let Some(notice) = self.conflict_notice.clone() {
            notice
        } else {
            "Shortcuts must be unique.".into()
        };

        let scrollbar = self.render_scrollbar(&theme, cx);
        div()
            .id("shortcuts-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(16.0, true, true, div()
                    .id("shortcuts-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .track_focus(&self.focus)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_start()
                                    .flex_wrap()
                                    .justify_between()
                                    .gap(px(24.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(200.0))
                                            .flex()
                                            .flex_col()
                                            .child(widgets::page_header(
                                                &theme,
                                                "Keyboard shortcuts",
                                                None,
                                            ))
                                            .child(
                                                widgets::page_subtitle(
                                                    &theme,
                                                    "Click a binding, then press the new key combination.",
                                                )
                                                .max_w(px(512.0))
                                                .line_height(px(20.0)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(self.render_key_search_button(&theme, cx))
                                            .child({
                                                // `disabled:opacity-35` when nothing is
                                                // customized or while recording.
                                                let disabled = !customized || recording.is_some();
                                                widgets::ghost_action(&theme)
                                                    .id("shortcuts-restore-defaults")
                                                    .flex_none()
                                                    .when(disabled, |el| el.opacity(0.35))
                                                    .when(!disabled, |el| {
                                                        el.on_click(
                                                            cx.listener(|this, _, _, cx| {
                                                                this.keymap = KeymapConfig::default();
                                                                this.stop_recording();
                                                                this.stop_key_search();
                                                                this.conflict_notice = None;
                                                                this.commit(cx);
                                                                this.set_escape_stops_active_agent(
                                                                    false, cx,
                                                                );
                                                                this.set_composer_send_behavior(
                                                                    ComposerSendBehavior::Enter,
                                                                    cx,
                                                                );
                                                            }),
                                                        )
                                                    })
                                                    .child(
                                                        crate::icons::icon(crate::icons::RESTART)
                                                            .size(px(14.0))
                                                            .text_color(theme.text_muted),
                                                    )
                                                    .child(SharedString::from("Restore defaults"))
                                            }),
                                    ),
                            )
                            .children(key_search)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .children(groups),
                            )
                            .child(
                                div()
                                    .mt(px(12.0))
                                    .px(px(4.0))
                                    .min_h(px(20.0))
                                    .flex()
                                    .justify_center()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .child(helper),
                            )
                            ,
                    )).fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn voice_shortcut_records_on_its_own_page_and_shortcuts_reads_it_back(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        assert_eq!(ShortcutId::ToggleDictation.default_combo(), "mod-d");
        assert_eq!(group(ShortcutId::ToggleDictation), "Voice");
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let (field, cx) =
            cx.add_window_view(|_, cx| ShortcutField::new(ShortcutId::ToggleDictation, cx));
        // A chord another shortcut owns is refused and the binding is kept.
        field.update_in(cx, |field, window, cx| field.start_recording(window, cx));
        cx.simulate_keystrokes(&format!("{modifier}-b"));
        field.update(cx, |field, cx| {
            assert!(!field.recording);
            assert!(field.notice.is_some());
            assert_eq!(
                crate::settings::current(cx).keymap.toggle_dictation,
                "mod-d"
            );
        });
        field.update_in(cx, |field, window, cx| field.start_recording(window, cx));
        cx.simulate_keystrokes(&format!("{modifier}-shift-v"));
        field.update(cx, |field, cx| {
            assert!(field.notice.is_none());
            assert_eq!(
                crate::settings::current(cx).keymap.toggle_dictation,
                "mod-shift-v"
            );
        });
        // Closing the window mid-recording (the app keeps running) must not
        // leave Appshots' capture shortcut suspended.
        field.update_in(cx, |field, window, cx| field.start_recording(window, cx));
        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
        field.update(cx, |field, _| assert!(!field.recording));
        // Elsewhere, a clash with a feature page's shortcut says where it lives.
        let notice = refusal(
            &KeymapConfig::default(),
            ComposerSendBehavior::Enter,
            ShortcutId::ToggleSidebar,
            "mod-d",
        )
        .unwrap();
        assert!(
            notice.ends_with("Hold to dictate in Settings → Voice."),
            "{notice}"
        );
        // Settings → Shortcuts, opened later with an older copy, adopts the
        // store's binding so its own commits cannot revert Voice's.
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::Enter,
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear());
        page.update(cx, |page, _| {
            assert_eq!(page.keymap.toggle_dictation, "mod-shift-v");
        });
    }

    #[gpui::test]
    fn conversation_controls_work_after_moving_out_of_shortcuts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::Enter,
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            );
            page.show_section(false, true);
            page
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let send = cx.debug_bounds("composer-send-behavior").unwrap();
        cx.simulate_click(send.center(), gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        let option = cx.debug_bounds("composer-send-behavior-option-1").unwrap();
        cx.simulate_click(option.center(), gpui::Modifiers::default());
        page.update(cx, |page, _| assert!(!page.send_select.is_open()));
        // Let the menu's exit animation finish before clicking beneath it:
        // the reap compares wall-clock instants, so real time must pass too.
        std::thread::sleep(
            crate::motion::MENU_OUT
                .total()
                .mul_f32(crate::motion::speed_scale()),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("composer-send-behavior-option-1").is_none());
        let escape = cx.debug_bounds("escape-stops-active-agent-toggle").unwrap();
        cx.simulate_click(escape.center(), gpui::Modifiers::default());
        page.update(cx, |page, _| {
            assert_eq!(page.composer_send_behavior, ComposerSendBehavior::ModEnter);
            assert!(page.escape_stops_active_agent);
            page.show_section(false, false);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("composer-send-behavior").is_none());
        assert!(
            cx.debug_bounds("escape-stops-active-agent-toggle")
                .is_none()
        );
    }

    #[gpui::test]
    fn appshots_setup_can_be_enabled_and_configured_by_keyboard(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::default(),
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            );
            page.show_appshots(true);
            page
        });
        window
            .update(cx, |page, w, cx| w.focus(&page.focus, cx))
            .unwrap();
        cx.update_window(window.into(), |_, w, cx| {
            w.draw(cx).clear();
        })
        .unwrap();
        let press = |cx: &mut gpui::TestAppContext, key: &str| {
            cx.update_window(window.into(), |_, w, cx| {
                w.draw(cx).clear();
                let keystroke = gpui::Keystroke::parse(key).unwrap();
                w.dispatch_event(
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke: keystroke.clone(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                w.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
                    cx,
                );
            })
            .unwrap();
        };
        press(cx, "tab");
        press(cx, "space");
        window
            .update(cx, |page, _, _| assert!(page.appshots_enabled))
            .unwrap();
        press(cx, "tab");
        press(cx, "enter");
        window
            .update(cx, |page, _, _| assert!(page.appshot_sound_enabled))
            .unwrap();
        // Skip the shortcut trigger; arrows open the destination dropdown on
        // the current choice and Enter commits the highlighted one.
        for key in ["tab", "tab", "down", "down", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::LastSession);
                assert!(!page.destination_select.is_open());
            })
            .unwrap();
        for key in ["space", "down", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::NewSession)
            })
            .unwrap();
        // Escape closes without choosing.
        for key in ["up", "up", "escape"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::NewSession);
                assert!(!page.destination_select.is_open());
            })
            .unwrap();
        for key in ["up", "up", "enter"] {
            press(cx, key);
        }
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.appshot_destination, AppshotDestination::LastSession)
            })
            .unwrap();
    }

    #[gpui::test]
    fn recorder_refuses_bound_actions_before_they_can_run(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let fired = Rc::new(Cell::new(false));
        let observed = fired.clone();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            cx.bind_keys([gpui::KeyBinding::new(
                &crate::settings::platform_combo("mod-n"),
                crate::shell::NewSession,
                None,
            )]);
            cx.on_action(move |_: &crate::shell::NewSession, _| observed.set(true));
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::default(),
                false,
                true,
                AppshotDestination::Automatic,
                cx,
            )
        });
        window
            .update(cx, |page, window, cx| {
                page.start_recording(ShortcutId::CaptureAppshot, window, cx)
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), &crate::settings::platform_combo("mod-n"));
        window
            .update(cx, |page, _, _| {
                assert!(!fired.get(), "the existing action ran while recording");
                assert!(
                    page.conflict_notice
                        .as_deref()
                        .unwrap()
                        .contains("New session")
                );
                assert_eq!(
                    page.keymap.capture_appshot,
                    ShortcutId::CaptureAppshot.default_combo()
                );
                assert!(page.recording.is_none());
                assert!(page.recording_interceptor.is_none());
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), &crate::settings::platform_combo("mod-n"));
        assert!(
            fired.get(),
            "finishing recording must restore normal actions"
        );
    }

    /// Install `keymap` the way the app does, so key searches read the
    /// bindings gpui would really dispatch.
    fn install_keymap(cx: &mut gpui::TestAppContext, keymap: &KeymapConfig) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::shell::apply_keymap(cx, keymap, ComposerSendBehavior::default());
        });
    }

    fn search(cx: &mut gpui::TestAppContext, keymap: &KeymapConfig, combo: &str) -> Vec<KeyUse> {
        let keystroke = Keystroke::parse(&crate::settings::platform_combo(combo)).unwrap();
        cx.update(|cx| {
            let keymap_bindings = cx.key_bindings();
            let keymap_bindings = keymap_bindings.borrow();
            key_uses(keymap_bindings.bindings(), keymap, &keystroke)
        })
    }

    fn rows(uses: &[KeyUse]) -> Vec<ShortcutId> {
        uses.iter().filter_map(|found| found.shortcut).collect()
    }

    #[gpui::test]
    fn key_search_reads_the_live_keymap(cx: &mut gpui::TestAppContext) {
        let mut keymap = KeymapConfig::default();
        keymap.set(ShortcutId::ToggleSidebar, "mod-shift-x".into());
        install_keymap(cx, &keymap);
        // A rebound shortcut is found under its new combination only.
        assert_eq!(
            rows(&search(cx, &keymap, "mod-shift-x")),
            [ShortcutId::ToggleSidebar]
        );
        assert!(!rows(&search(cx, &keymap, "mod-b")).contains(&ShortcutId::ToggleSidebar));
        // Every rebindable shortcut maps back to its own row from what
        // `apply_keymap` bound; one missing from `shortcut_action` would
        // read as a fixed binding instead.
        for id in ShortcutId::ALL.into_iter().filter(|id| id.available()) {
            assert!(
                rows(&search(cx, &keymap, keymap.get(id))).contains(&id),
                "{id:?}"
            );
        }
        // Fixed bindings come by a plain name and where they work.
        assert_eq!(
            search(cx, &keymap, "mod-k"),
            [KeyUse {
                shortcut: None,
                name: "Toggle command palette".into(),
                places: Vec::new(),
            }]
        );
        let select_all = search(cx, &keymap, "mod-a");
        let select_all = select_all
            .iter()
            .find(|found| found.name == "Select all")
            .unwrap();
        assert!(
            select_all.places.contains(&"Composer".to_string()),
            "{select_all:?}"
        );
        // Nothing uses this one.
        assert!(search(cx, &keymap, "mod-alt-shift-y").is_empty());
    }

    #[gpui::test]
    fn show_home_default_is_free_in_the_default_keymap(cx: &mut gpui::TestAppContext) {
        let keymap = KeymapConfig::default();
        install_keymap(cx, &keymap);
        assert_eq!(
            search(cx, &keymap, ShortcutId::ShowHome.default_combo()),
            [KeyUse {
                shortcut: Some(ShortcutId::ShowHome),
                name: "Show Home".into(),
                places: Vec::new(),
            }]
        );
    }

    #[gpui::test]
    fn key_search_names_a_combination_without_running_it(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let fired = Rc::new(Cell::new(false));
        let observed = fired.clone();
        install_keymap(cx, &KeymapConfig::default());
        cx.update(|cx| {
            cx.on_action(move |_: &crate::shell::NewSession, _| observed.set(true));
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            ShortcutsPage::new(
                state,
                KeymapConfig::default(),
                false,
                ComposerSendBehavior::default(),
                false,
                false,
                AppshotDestination::Automatic,
                cx,
            )
        });
        let new_session = crate::settings::platform_combo("mod-n");
        let click_search = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear());
            let button = cx.debug_bounds("shortcuts-key-search").unwrap();
            cx.simulate_click(button.center(), gpui::Modifiers::default());
        };
        click_search(cx);
        cx.simulate_keystrokes(&new_session);
        page.update(cx, |page, _| {
            let search = page.key_search.as_ref().expect("still listening");
            assert_eq!(search.combo.as_deref(), Some("mod-n"));
            assert!(rows(&search.uses).contains(&ShortcutId::NewSession));
        });
        assert!(!fired.get(), "the searched action ran");
        // Pressing the button again ends it; so does Escape.
        click_search(cx);
        page.update(cx, |page, _| assert!(page.key_search.is_none()));
        click_search(cx);
        cx.simulate_keystrokes("escape");
        page.update(cx, |page, _| assert!(page.key_search.is_none()));
        cx.simulate_keystrokes(&new_session);
        assert!(fired.get(), "ending the search must restore normal actions");
    }

    fn new_page(cx: &mut Context<ShortcutsPage>) -> ShortcutsPage {
        let state = cx.new(|_| AppState::new());
        ShortcutsPage::new(
            state,
            KeymapConfig::default(),
            false,
            ComposerSendBehavior::default(),
            false,
            false,
            AppshotDestination::Automatic,
            cx,
        )
    }

    #[gpui::test]
    fn key_search_listens_until_the_owner_leaves(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let fired = Rc::new(Cell::new(false));
        let observed = fired.clone();
        install_keymap(cx, &KeymapConfig::default());
        cx.update(|cx| {
            cx.on_action(move |_: &crate::shell::NewSession, _| observed.set(true));
        });
        let (page, cx) = cx.add_window_view(|_, cx| new_page(cx));
        let none = gpui::Modifiers::default();
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear();
        });
        cx.run_until_parked();
        let button = cx.debug_bounds("shortcuts-key-search").unwrap().center();
        cx.simulate_click(button, none);
        // A right-click on the lit button, or a press dragged off it, must
        // not pull focus from the page and leave a search that hears nothing.
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_mouse_down(button, gpui::MouseButton::Right, none);
        cx.simulate_mouse_up(button, gpui::MouseButton::Right, none);
        cx.simulate_mouse_down(button, gpui::MouseButton::Left, none);
        cx.simulate_mouse_up(gpui::point(px(1.0), px(1.0)), gpui::MouseButton::Left, none);
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-n"));
        assert!(!fired.get(), "the searched action ran");
        page.update(cx, |page, _| {
            let search = page.key_search.as_ref().expect("still listening");
            assert_eq!(search.combo.as_deref(), Some("mod-n"));
        });
        // Leaving the window ends it, and with it the hold on Appshots'
        // hotkey, which must work in the app the owner went to.
        assert!(cx.update(|window, _| window.is_window_active()));
        cx.deactivate_window();
        page.update(cx, |page, _| assert!(page.key_search.is_none()));
    }

    #[gpui::test]
    fn key_search_answer_stays_on_screen(cx: &mut gpui::TestAppContext) {
        install_keymap(cx, &KeymapConfig::default());
        let (page, cx) = cx.add_window_view(|_, cx| new_page(cx));
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        // A reveal scrolls while painting and asks for one more frame.
        let draw = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..2 {
                cx.update(|window, cx| window.draw(cx).clear());
            }
        };
        let answer_on_screen = |cx: &mut gpui::VisualTestContext| {
            draw(cx);
            let results = cx.debug_bounds("shortcuts-key-search-results").unwrap();
            let view = page.update(cx, |page, _| page.scroll.scroll.bounds());
            results.top() >= view.top() && results.bottom() <= view.bottom()
        };
        draw(cx);
        let button = cx.debug_bounds("shortcuts-key-search").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        // The last jump slot is the foot of the table: the page scrolls to it.
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-9"));
        draw(cx);
        assert!(page.update(cx, |page, _| page.scroll.scroll.offset().y) < px(0.0));
        // A combination no row has, or nothing has, scrolls its answer back.
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-k"));
        assert!(answer_on_screen(cx));
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-9"));
        draw(cx);
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-alt-shift-y"));
        assert!(answer_on_screen(cx));
        // A hit row near the top (Random wallpaper) keeps the answer above it
        // in view, even coming back from the foot of the table.
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-9"));
        draw(cx);
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-u"));
        assert!(answer_on_screen(cx));
    }

    #[test]
    fn recording_outcomes() {
        assert_eq!(
            record_key("escape", false, false, false, false),
            RecordOutcome::Cancelled
        );
        assert_eq!(
            record_key("Escape", true, false, false, false),
            RecordOutcome::Cancelled
        );
        assert_eq!(
            record_key("s", false, false, false, true),
            RecordOutcome::Set("mod-s".into())
        );
        assert_eq!(
            record_key("k", false, true, true, true),
            RecordOutcome::Set("mod-alt-shift-k".into())
        );
        // macOS-only: elsewhere ctrl IS the primary and records as "mod".
        #[cfg(target_os = "macos")]
        assert_eq!(
            record_key("tab", true, false, true, false),
            RecordOutcome::Set("ctrl-shift-tab".into())
        );
        // Bare modifiers stay recording.
        assert_eq!(
            record_key("shift", false, false, true, false),
            RecordOutcome::Ignored
        );
        assert_eq!(
            record_key("ctrl", true, false, false, false),
            RecordOutcome::Ignored
        );
    }

    #[test]
    fn every_shortcut_lands_in_a_rendered_group() {
        // The page renders GROUP_ORDER's cards and nothing else — a group()
        // arm returning a name missing from GROUP_ORDER would silently drop
        // its rows from Settings.
        for id in ShortcutId::ALL {
            assert!(
                GROUP_ORDER.contains(&group(id)),
                "{:?} is grouped under {:?}, which GROUP_ORDER does not render",
                id,
                group(id)
            );
        }
        // And every named group has at least one row — no empty cards.
        for name in GROUP_ORDER {
            assert!(
                ShortcutId::ALL.into_iter().any(|id| group(id) == name),
                "group {:?} would render an empty card",
                name
            );
        }
    }

    #[test]
    fn conflicting_records_are_refused() {
        // zeron parity: a combo bound elsewhere is refused at record time (the
        // helper names the owner) — conflicts never persist into the keymap.
        let keymap = KeymapConfig::default();
        let RecordOutcome::Set(combo) = record_key("r", false, false, false, true) else {
            panic!("expected Set");
        };
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleSidebar, &combo),
            Some(ShortcutId::ToggleChanges)
        );
        // Re-recording a shortcut's own combo is not a conflict.
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleChanges, &combo),
            None
        );
        // A free combo conflicts with nothing.
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::ToggleSidebar, "mod-shift-x"),
            None
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn appshot_binding_participates_in_existing_conflict_checks() {
        let mut keymap = KeymapConfig::default();
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::CaptureAppshot, "mod-n"),
            Some(ShortcutId::NewSession)
        );
        keymap.set(ShortcutId::CaptureAppshot, "mod-alt-k".into());
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::NewSession, "mod-alt-k"),
            Some(ShortcutId::CaptureAppshot)
        );
        assert_eq!(
            conflict_owner(&keymap, ShortcutId::CaptureAppshot, "mod-alt-k"),
            None
        );
    }

    #[test]
    fn modifier_send_labels_are_platform_specific() {
        assert_eq!(modifier_send_label(true), "⌘ Enter");
        assert_eq!(modifier_send_label(false), "Ctrl Enter");
    }

    #[test]
    fn modifier_send_is_always_reserved_for_the_composer() {
        assert!(send_combo_is_reserved(
            ComposerSendBehavior::Enter,
            "mod-enter"
        ));
        assert!(send_combo_is_reserved(
            ComposerSendBehavior::ModEnter,
            "mod-enter"
        ));
        assert!(!send_combo_is_reserved(
            ComposerSendBehavior::ModEnter,
            "mod-shift-enter"
        ));
    }
}
