//! Drawing Home: the toolbar (with quiet widgets' chips), the Customize
//! tray, and the grid of cards with one body per widget kind. Colors come
//! from the theme, so light and dark both follow it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDate, Utc};
use gpui::{
    AnimationExt as _, AnyElement, AnyView, App, ClickEvent, Context, CursorStyle, DispatchPhase,
    Div, ElementId, EntityId, FontWeight, HitboxBehavior, Hsla, MouseButton, MouseMoveEvent, Role,
    SharedString, Stateful, Toggled, Window, div, prelude::*, px, relative,
};
use keron_home::kinds::{is_openable, parse_chat_link, parse_date, parse_time, short_age};
use keron_home::loose_ends::{Action, snooze_choices};
use keron_home::{AgendaItem, DeviceItem, Heat, Kind, Stat, TimelineItem};
use zeron_theme::AccentPreset;

use super::{
    Body, CardDragPayload, Drawn, Home, Leaving, ListItem, Manifest, Mode, Payload, RowChange,
    RowUi, SignIn, Slot, SourceSpec, WidgetState, dense_cells, drawn_list_rows, is_door, limit,
    visible_list_rows,
};
use crate::icons::{self, icon};
use crate::motion::{self, MotionSpec};
use crate::popover;
use crate::settings::widgets::{self, ActionTone};
use crate::theme::Theme;
use crate::typography::ui_rems;

const CARD_RADIUS: f32 = 12.0;
/// One more column each time every card can get at least this much.
const CARD_MIN_WIDTH: f32 = 280.0;
/// Most columns Home lays out, however wide the window.
const MAX_COLUMNS: u16 = 4;
const GRID_GAP: f32 = 10.0;
/// Room left under Home for the update notice at the window's bottom.
const BOTTOM_CLEARANCE: f32 = 64.0;
/// Burning heat breathes this slowly.
const HEAT_PULSE: MotionSpec = MotionSpec::new(1800, motion::EASE_IN_OUT);
/// Space between a list card's rows.
const LIST_GAP: f32 = 6.0;
/// The "+N more" line under a list, in pixels at the default text size.
const MORE_LINE_HEIGHT: f32 = 15.0;
/// How far a row's hover wash reaches past the text column on each side.
const ROW_INSET: f32 = 6.0;
/// A row action button's square side, and its glyph.
const ROW_ACTION_SIZE: f32 = 22.0;
const ROW_ACTION_ICON_SIZE: f32 = 14.0;
const ROW_ACTION_GAP: f32 = 2.0;
/// Space between a row action's tooltip and the top of its row.
const ROW_TOOLTIP_GAP: f32 = 2.0;
/// How far above the pointer the tooltip ends when the row hasn't been
/// measured: clear of the button wherever the pointer is in it.
const ROW_TOOLTIP_FALLBACK_LIFT: f32 = 24.0;
/// A quiet widget's chip in the toolbar: its side, its glyph, the space
/// between chips, and how small it starts as it fades in.
const CHIP_SIZE: f32 = 26.0;
const CHIP_ICON_SIZE: f32 = 14.0;
const CHIP_GAP: f32 = 2.0;
const CHIP_START_SCALE: f32 = 0.8;
/// How far below its place a card that left its chip starts.
const CARD_RISE: f32 = 6.0;

/// The pointer ghost while a card drags: nothing, the card itself moves.
struct CardGhost;

impl Render for CardGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Render for Home {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.frame;
        let width = (frame.width - 2.0 * Theme::SPACE_LG).max(0.0);
        if !self.loaded || width <= 0.0 {
            return div().into_any_element();
        }
        self.keyboard_row = self.keyboard_focused_row(window, cx);
        self.sync_quiet(cx);
        // Rows closing up after Done, and chips and cards fading in where
        // they moved, are drawn from the clock, frame by frame.
        if self.widgets.values().any(|state| !state.leaving.is_empty())
            || !self.chip_in.is_empty()
            || !self.card_in.is_empty()
        {
            window.request_animation_frame();
        }
        let theme = Theme::of(cx).clone();
        let columns = (((width + GRID_GAP) / (CARD_MIN_WIDTH + GRID_GAP)).floor() as u16)
            .clamp(1, MAX_COLUMNS);
        let toolbar = self.render_toolbar(&theme, cx);
        let tray = self.customize.then(|| self.render_tray(&theme, cx));
        let grid = self.render_grid(&theme, columns, cx);
        let content = div()
            .w(px(width))
            .mx_auto()
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(GRID_GAP))
            .child(toolbar)
            .children(tray)
            .children(grid);
        div()
            .relative()
            .size_full()
            .opacity(frame.opacity)
            .child(
                div()
                    .absolute()
                    .top(px(frame.top))
                    .left_0()
                    .right_0()
                    .bottom(px(BOTTOM_CLEARANCE))
                    .child(
                        crate::edge_fade::edge_faded(
                            16.0,
                            true,
                            true,
                            div()
                                .id("home-scroll")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .child(content),
                        )
                        .fade_overflow_y(&self.scroll),
                    ),
            )
            .into_any_element()
    }
}

impl Home {
    // ---- toolbar ----

    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.customize;
        let customize = button(theme, "home-customize", ActionTone::Quiet)
            .aria_label("Customize Home")
            .aria_expanded(open)
            .when(open, |el| el.bg(theme.glass_hover()).text_color(theme.text))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_customize(cx)))
            .child(icon(icons::WIDGET).size(px(13.0)).text_color(if open {
                theme.text
            } else {
                theme.text_muted
            }))
            .child("Customize");
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_h(px(32.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .children(self.render_sign_in(theme, cx)),
            )
            .children(self.render_chips(theme, cx))
            .child(customize)
            .into_any_element()
    }

    /// Quiet widgets as icon chips, in Home's order.
    fn render_chips(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let chips = self.chip_slots();
        if chips.is_empty() {
            return None;
        }
        let chips: Vec<AnyElement> = chips
            .iter()
            .map(|slot| self.render_chip(theme, &slot.manifest, cx))
            .collect();
        Some(
            // Shrinks and wraps onto more rows before it would push
            // Customize out of the toolbar.
            div()
                .flex_shrink_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap(px(CHIP_GAP))
                .children(chips)
                .into_any_element(),
        )
    }

    /// A quiet widget's chip: its icon in a quiet round button. The tooltip
    /// says what the card would; a click opens the card in the grid until
    /// the next click, and the chip stays pressed meanwhile.
    fn render_chip(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = manifest.id.clone();
        let peeked = self.peeks.contains(&id);
        let nothing = empty_text(manifest);
        let mut tip = format!("{} · {nothing}", manifest.title);
        if let Some(note) = self
            .widgets
            .get(&id)
            .and_then(|state| state.payload.as_ref())
            .and_then(|payload| freshness(manifest, payload))
        {
            tip.push_str(" · ");
            tip.push_str(&note);
        }
        let label = format!(
            "{}: {nothing}. {}",
            manifest.title,
            if peeked { "Hide card" } else { "Show card" }
        );
        let key = format!("home-chip-{id}");
        let fade_key = format!("{key}-hover");
        let wash = theme.glass_hover();
        let (bg, ink) = if peeked {
            (wash, theme.text)
        } else {
            (
                motion::hover_blend(&fade_key, wash.opacity(0.0), wash),
                motion::hover_blend(&fade_key, theme.text_muted, theme.text),
            )
        };
        // Just moved here from the grid: it grows a little as it fades in.
        let t = Home::swap_t(self.chip_in.get(&id)).unwrap_or(1.0);
        let scale = CHIP_START_SCALE + (1.0 - CHIP_START_SCALE) * t;
        let accent = theme.accent;
        div()
            .id(SharedString::from(key))
            .flex_none()
            .size(px(CHIP_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .on_hover(motion::hover_listener(fade_key))
            .tab_index(0)
            .role(Role::Button)
            .aria_label(label)
            .aria_expanded(peeked)
            .focus_visible(move |s| s.border_1().border_color(accent))
            .tooltip(widgets::text_tooltip(tip))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_peek(&id, cx)))
            .child(
                div()
                    .size(px(CHIP_SIZE * scale))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(bg)
                    .opacity(t)
                    .child(
                        icon(icon_for(manifest))
                            .size(px(CHIP_ICON_SIZE * scale))
                            .text_color(ink),
                    ),
            )
            .into_any_element()
    }

    /// The one place to connect the door, shown while a shown widget needs
    /// it and it isn't connected.
    fn render_sign_in(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let needs_door = self
            .arranged()
            .iter()
            .any(|slot| slot.shown && is_door(&slot.manifest.source));
        if !needs_door {
            return None;
        }
        let view = cx.entity_id();
        let line = |text: SharedString, color: Hsla| {
            div()
                .min_w_0()
                .truncate()
                .text_size(ui_rems(12.5))
                .text_color(color)
                .child(text)
        };
        let row = div().flex().items_center().gap(px(8.0)).min_w_0();
        let spinner = |cx: &mut Context<Self>| -> AnyElement {
            crate::loaders::mini_mono_spinner("home-sign-in", 2.0, theme.text_muted, view, cx)
                .into_any_element()
        };
        let element = match &self.sign_in {
            SignIn::Checking | SignIn::NoDoor | SignIn::SignedIn => return None,
            SignIn::SignedOut => row
                .child(line(
                    "Your memory isn't connected.".into(),
                    theme.text_muted,
                ))
                .child(
                    button(theme, "home-connect", ActionTone::Filled)
                        .aria_label("Connect your memory")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx)))
                        .child("Connect your memory"),
                ),
            SignIn::Opening => row
                .child(spinner(cx))
                .child(line("Opening the browser…".into(), theme.text_muted)),
            SignIn::Waiting => row
                .child(spinner(cx))
                .child(line("Waiting for the browser…".into(), theme.text_muted))
                .child(
                    button(theme, "home-connect-cancel", ActionTone::Quiet)
                        .aria_label("Cancel the sign-in")
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_sign_in(cx)),
                        )
                        .child("Cancel"),
                ),
            SignIn::Failed(message) => row
                .child(
                    icon(icons::DANGER_TRIANGLE)
                        .size(px(13.0))
                        .text_color(theme.warning_muted),
                )
                .child(line(message.clone().into(), theme.warning_muted))
                .child(
                    button(theme, "home-connect-again", ActionTone::Filled)
                        .aria_label("Try connecting your memory again")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx)))
                        .child("Try again"),
                ),
        };
        Some(element.into_any_element())
    }

    // ---- Customize ----

    fn render_tray(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let order = self.arranged();
        let count = order.len();
        let rows: Vec<AnyElement> = order
            .iter()
            .enumerate()
            .map(|(ix, slot)| self.render_tray_row(theme, ix, count, slot, cx))
            .collect();
        let problems = self
            .catalog
            .problems
            .iter()
            .map(|problem| problem.to_string())
            .chain(
                self.layout_error
                    .iter()
                    .map(|error| format!("{error}. Changes here aren't saved until it's fixed.")),
            )
            .map(|text| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.0))
                    .text_size(ui_rems(12.0))
                    .line_height(ui_rems(16.0))
                    .text_color(theme.warning_muted)
                    .child(
                        div().flex_none().mt(px(2.0)).child(
                            icon(icons::DANGER_TRIANGLE)
                                .size(px(12.0))
                                .text_color(theme.warning_muted),
                        ),
                    )
                    .child(div().min_w_0().child(text))
            });
        let tray = card_surface(theme)
            .id("home-tray")
            .px(px(14.0))
            .py(px(12.0))
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x(px(10.0))
                    .child(
                        div()
                            .text_size(ui_rems(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Widgets"),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("Show, hide, resize and reorder. Cards can be dragged too."),
                    ),
            )
            .child(div().flex().flex_col().children(rows))
            .child(self.render_collapse_switch(theme, cx))
            .children(problems)
            .children(self.render_add_builtins(theme, cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        button(theme, "home-describe", ActionTone::Outlined)
                            .aria_label("Describe a widget for an agent to make")
                            .on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.describe_widget(cx)),
                            )
                            .child("Describe a widget…"),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("An agent writes it, and Home picks it up by itself."),
                    ),
            );
        crate::frost::frosted(CARD_RADIUS, crate::frost::MENU_BLUR, tray).into_any_element()
    }

    /// "Collapse empty widgets": quiet widgets leave the grid for chips.
    fn render_collapse_switch(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.layout.collapse_empty;
        let accent = theme.accent;
        let switch = widgets::toggle_switch(theme, on, "home-collapse-empty")
            .id("home-collapse-empty")
            .tab_index(0)
            .role(Role::Switch)
            .aria_label("Collapse empty widgets")
            .aria_toggled(toggled(on))
            .focus_visible(move |s| s.border_2().border_color(accent))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_collapse_empty(!on, cx)),
            );
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pt(px(10.0))
            .border_t_1()
            .border_color(widgets::row_divider(theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(ui_rems(13.0))
                            .text_color(theme.text)
                            .child("Collapse empty widgets"),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(
                                "A widget with nothing to show waits as an icon beside Customize.",
                            ),
                    ),
            )
            .child(switch)
            .into_any_element()
    }

    /// "Add:" and a button per built-in with no file in the widgets folder.
    fn render_add_builtins(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let missing = self.missing_builtins();
        (!missing.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .text_size(ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Add:")
                .children(missing.into_iter().map(|(id, title)| {
                    small_button(theme, format!("home-add-{id}"))
                        .aria_label(format!("Add the {title} widget"))
                        .on_click(
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.add_builtin(id, cx)
                            }),
                        )
                        .child(SharedString::from(title))
                }))
        })
    }

    fn render_tray_row(
        &self,
        theme: &Theme,
        ix: usize,
        count: usize,
        slot: &Slot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = slot.manifest.id.clone();
        let title = slot.manifest.title.clone();
        let shown = slot.shown;
        let accent = theme.accent;
        let switch = widgets::toggle_switch(theme, shown, format!("home-shown-{id}"))
            .id(SharedString::from(format!("home-shown-{id}")))
            .tab_index(0)
            .role(Role::Switch)
            .aria_label(format!("Show {title}"))
            .aria_toggled(toggled(shown))
            .focus_visible(move |s| s.border_2().border_color(accent))
            .cursor_pointer()
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _: &ClickEvent, _, cx| this.set_shown(&id, !shown, cx)
            }));
        let width_choice = |columns: u8| {
            let selected = slot.width == columns;
            let label = if columns == 1 {
                format!("{title}: one column")
            } else {
                format!("{title}: two columns")
            };
            div()
                .id(SharedString::from(format!("home-width-{id}-{columns}")))
                .px(px(8.0))
                .py(px(1.0))
                .rounded(px(5.0))
                .text_size(ui_rems(11.5))
                .font_family(theme.font_mono.clone())
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .when(selected, |el| el.bg(theme.wash(0.1)))
                .cursor_pointer()
                .tab_index(0)
                .role(Role::RadioButton)
                .aria_label(label)
                .aria_toggled(toggled(selected))
                .focus_visible(move |s| s.border_1().border_color(accent))
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, _: &ClickEvent, _, cx| this.set_width(&id, columns, cx)
                }))
                .child(SharedString::from(columns.to_string()))
        };
        let widths = div()
            .flex()
            .flex_none()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(7.0))
            .bg(theme.wash(0.05))
            .child(width_choice(1))
            .child(width_choice(2));
        let hover = theme.glass_hover();
        let arrow = |delta: isize, glyph: &'static str, label: String, enabled: bool| {
            let base = div()
                .id(SharedString::from(format!("home-move-{id}-{delta}")))
                .flex_none()
                .size(px(26.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph).size(px(14.0)).text_color(theme.text_muted));
            if !enabled {
                return base.opacity(0.3);
            }
            let id = id.clone();
            base.cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tab_index(0)
                .role(Role::Button)
                .aria_label(label)
                .focus_visible(move |s| s.border_1().border_color(accent))
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.move_by(&id, delta, cx)),
                )
        };
        let up = arrow(-1, icons::ALT_ARROW_UP, format!("Move {title} up"), ix > 0);
        let down = arrow(
            1,
            icons::ALT_ARROW_DOWN,
            format!("Move {title} down"),
            ix + 1 < count,
        );
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_h(px(38.0))
            .when(ix > 0, |el| {
                el.border_t_1().border_color(widgets::row_divider(theme))
            })
            .child(
                icon(icon_for(&slot.manifest))
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_rems(13.0))
                    .text_color(if shown { theme.text } else { theme.text_muted })
                    .child(SharedString::from(title.clone())),
            )
            .child(widths)
            .child(up)
            .child(down)
            .child(switch)
            .into_any_element()
    }

    // ---- the grid ----

    /// The cards; nothing when every shown widget is a chip.
    fn render_grid(
        &self,
        theme: &Theme,
        columns: u16,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let slots = self.grid_slots();
        if slots.is_empty() && !self.chip_slots().is_empty() {
            return None;
        }
        if slots.is_empty() {
            let text = if self.catalog.manifests.is_empty() {
                "No widgets yet. Describe one in Customize."
            } else {
                "Every widget is hidden. Turn some on in Customize."
            };
            return Some(
                div()
                    .py(px(8.0))
                    .text_size(ui_rems(12.5))
                    .text_color(theme.text_muted)
                    .child(text)
                    .into_any_element(),
            );
        }
        let count = slots.len();
        // While a card drags, the others make room: the order shown is the
        // order it would drop into.
        let (order, previous) = match &self.drag {
            Some(drag) => (
                moved(count, drag.from, drag.over),
                moved(count, drag.from, drag.prev_over),
            ),
            None => ((0..count).collect(), (0..count).collect()),
        };
        let widths: Vec<u8> = order
            .iter()
            .map(|&ix| {
                if columns > 1 {
                    slots[ix].width.clamp(1, 2)
                } else {
                    1
                }
            })
            .collect();
        let cells = dense_cells(&widths, columns);
        let reduced = motion::reduced_motion(cx);
        let mut cards = Vec::with_capacity(count);
        for (position, &ix) in order.iter().enumerate() {
            let slot = &slots[ix];
            let (row, col) = cells[position];
            let span = u16::from(widths[position]);
            let mut card = self
                .render_card(theme, slot, ix, cx)
                .row_start(row as i16 + 1)
                .col_start(col as i16 + 1)
                .col_end((col + span) as i16 + 1);
            // Just out of its chip: it fades in and rises into place.
            let fade_in = Home::swap_t(self.card_in.get(&slot.manifest.id));
            if let Some(t) = fade_in {
                card = card.opacity(t).top(px(CARD_RISE * (1.0 - t)));
            }
            // A card that moved slides from its old slot to its new one.
            let slide = self.drag.as_ref().and_then(|drag| {
                let before = previous.iter().position(|&other| other == ix)?;
                if before == position {
                    return None;
                }
                let from = drag.slots.get(before)?.origin;
                let to = drag.slots.get(position)?.origin;
                Some((
                    f32::from(from.x - to.x),
                    f32::from(from.y - to.y),
                    drag.epoch,
                ))
            });
            let card = match slide {
                Some((dx, dy, epoch)) if !reduced => card
                    .with_animation(
                        ElementId::Name(format!("home-slide-{}-{epoch}", slot.manifest.id).into()),
                        motion::TAB_SLIDE.animation(),
                        move |el, t| {
                            el.relative()
                                .left(px(dx * (1.0 - t)))
                                .top(px(dy * (1.0 - t)))
                        },
                    )
                    .into_any_element(),
                _ => card.into_any_element(),
            };
            // The backdrop blur ignores opacity, so it fades in with the card.
            let blur = crate::frost::MENU_BLUR * fade_in.unwrap_or(1.0);
            cards.push(crate::frost::frosted(CARD_RADIUS, blur, card).into_any_element());
        }
        let grid = div()
            .id("home-grid")
            .w_full()
            .grid()
            .grid_cols(columns)
            // Each card is as tall as its content, not its row's tallest card.
            .items_start()
            .gap(px(GRID_GAP))
            .when(self.customize, |grid| {
                grid.on_drag_move::<CardDragPayload>(cx.listener(
                    |this, event: &gpui::DragMoveEvent<CardDragPayload>, _, cx| {
                        let (id, from) = {
                            let payload = event.drag(cx);
                            (payload.id.clone(), payload.from)
                        };
                        this.drag_over(&id, from, event.event.position, cx);
                    },
                ))
                .on_drop::<CardDragPayload>(
                    cx.listener(|this, _: &CardDragPayload, _, cx| this.drop_card(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.cancel_drag(cx)),
                )
            })
            .children(cards)
            .into_any_element();
        Some(grid)
    }

    fn render_card(
        &self,
        theme: &Theme,
        slot: &Slot,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let manifest = &slot.manifest;
        let id = manifest.id.clone();
        let state = self.widgets.get(&id);
        let dragging = self.drag.as_ref().is_some_and(|drag| drag.id == id);
        let header = self.render_header(theme, manifest, state);
        let body = self.render_body(theme, manifest, state, cx);
        let footer = render_footer(theme, manifest, state);
        let mut card = card_surface(theme)
            .id(SharedString::from(format!("home-card-{id}")))
            .min_w_0()
            .px(px(12.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .gap(px(8.0))
            .role(Role::Group)
            .aria_label(manifest.title.clone())
            .when(dragging, |el| el.border_color(theme.accent.opacity(0.7)))
            .child(header)
            .child(body)
            .children(footer);
        if self.customize {
            let bounds = self.card_bounds.clone();
            let key = id.clone();
            card = card
                .cursor(CursorStyle::OpenHand)
                .child(
                    gpui::canvas(
                        move |measured, _, _| {
                            bounds.borrow_mut().insert(key, measured);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .on_drag(
                    CardDragPayload { id, from: index },
                    |_payload, _point, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| CardGhost)
                    },
                );
        }
        card
    }

    fn render_header(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: Option<&WidgetState>,
    ) -> Div {
        let (count, hot) = card_count(state);
        div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .min_w_0()
            .child(
                icon(icon_for(manifest))
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_rems(12.5))
                    .line_height(ui_rems(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(manifest.title.clone())),
            )
            .children(count.map(|count| {
                let chip = div()
                    .flex_none()
                    .text_size(ui_rems(11.0))
                    .font_weight(FontWeight::MEDIUM);
                // Loose ends go amber once anything is hot or burning.
                let chip = if hot {
                    chip.px(px(5.0))
                        .rounded(px(4.0))
                        .bg(theme.warning.opacity(0.16))
                        .text_color(theme.warning)
                } else {
                    chip.text_color(theme.text_faint)
                };
                chip.child(SharedString::from(count.to_string()))
            }))
            .child(div().flex_1())
            .when(self.customize, |el| {
                el.child(
                    icon(icons::DRAG_HANDLE)
                        .size(px(14.0))
                        .text_color(theme.text_faint),
                )
            })
    }

    fn render_body(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: Option<&WidgetState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity_id();
        if is_door(&manifest.source) {
            match &self.sign_in {
                SignIn::SignedIn => {}
                SignIn::Checking => return skeleton(theme, view, cx),
                SignIn::NoDoor => return muted(theme, "No memory door is set up."),
                _ => return muted(theme, "Connect your memory to see this."),
            }
        }
        if matches!(manifest.source, SourceSpec::Script(_)) && !self.scripts_ready {
            return skeleton(theme, view, cx);
        }
        let Some(state) = state else {
            return skeleton(theme, view, cx);
        };
        if let Some(reason) = &state.unavailable {
            return muted(theme, reason.clone());
        }
        let error = state
            .error
            .as_ref()
            .map(|message| self.render_error(theme, manifest, message, cx));
        let content = match &state.payload {
            Some(payload) => Some(self.render_payload(theme, manifest, state, payload, view, cx)),
            None if error.is_none() => return skeleton(theme, view, cx),
            None => None,
        };
        let action_error = state.action_error.as_ref().map(|(message, _)| {
            div()
                .text_size(ui_rems(12.0))
                .line_height(ui_rems(16.0))
                .text_color(theme.danger_muted)
                .child(SharedString::from(message.clone()))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(error)
            .children(content)
            .children(action_error)
            .into_any_element()
    }

    /// A compact warning with Retry; rows already shown stay under it.
    fn render_error(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        message: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let amber = theme.warning;
        let text = theme.warning_muted.opacity(0.9);
        let can_retry = matches!(self.mode(manifest), Mode::Fetch);
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(amber.opacity(0.2))
            .bg(amber.opacity(0.06))
            .child(icon(icons::DANGER_TRIANGLE).size(px(13.0)).text_color(text))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_rems(12.0))
                    .line_height(ui_rems(16.0))
                    .text_color(text)
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(SharedString::from(message.to_string())),
            )
            .when(can_retry, |el| {
                let id = manifest.id.clone();
                el.child(
                    small_button(theme, format!("home-retry-{id}"))
                        .aria_label(format!("Retry {}", manifest.title))
                        .on_click(
                            cx.listener(move |this, _: &ClickEvent, _, cx| this.retry(&id, cx)),
                        )
                        .child("Retry"),
                )
            })
            .into_any_element()
    }

    fn render_payload(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: &WidgetState,
        payload: &Payload,
        view: EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &payload.body {
            Body::List(items) => self.render_list(theme, manifest, state, items, view, cx),
            Body::Timeline(items) => render_timeline(theme, manifest, items),
            Body::Agenda(items) => self.render_agenda(theme, manifest, items, cx),
            Body::Devices(items) => render_devices(theme, manifest, items),
            Body::Stat(stat) => render_stat(theme, manifest, stat),
        }
    }

    // ---- list ----

    fn render_list(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: &WidgetState,
        items: &[ListItem],
        view: EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drawn = drawn_list_rows(
            items,
            &state.overrides,
            |id| state.leaving.contains_key(id),
            limit(manifest),
        );
        if drawn.rows.is_empty() {
            return empty(theme, manifest);
        }
        let rows: Vec<AnyElement> = drawn
            .rows
            .into_iter()
            .enumerate()
            .map(|(ix, (item, drawn))| {
                let row = self.render_list_row(theme, manifest, state, item, ix, view, cx);
                match drawn {
                    Drawn::Whole => row,
                    Drawn::Leaving => {
                        match item.id.as_deref().and_then(|id| state.leaving.get(id)) {
                            Some(leaving) => collapsing(leaving, row),
                            None => row,
                        }
                    }
                    Drawn::Joining { with } => match state.leaving.get(with) {
                        Some(leaving) => {
                            // Its own height once it has been drawn; until
                            // then, the height of the row it replaces.
                            let height = item
                                .id
                                .as_deref()
                                .and_then(|id| state.row_ui.borrow().get(id)?.bounds)
                                .map_or(leaving.height, |bounds| f32::from(bounds.size.height));
                            joining(leaving, height, row)
                        }
                        None => row,
                    },
                }
            })
            .collect();
        // The line goes with the last row past the limit, closing as it opens.
        let more = match drawn.more_closing {
            Some(count) => {
                let t = state
                    .leaving
                    .values()
                    .map(Leaving::progress)
                    .fold(0.0, f32::max);
                more_line(theme, count).map(|line| {
                    div()
                        .flex_none()
                        .overflow_hidden()
                        .h(ui_rems(MORE_LINE_HEIGHT * (1.0 - t)))
                        .mt(px(-LIST_GAP * t))
                        .opacity(1.0 - t)
                        .child(line)
                })
            }
            None => more_line(theme, drawn.more),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(LIST_GAP))
            .children(rows)
            .children(more)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_list_row(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: &WidgetState,
        item: &ListItem,
        ix: usize,
        view: EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let widget = manifest.id.as_str();
        // By id where the row has one, so its hover stays with it when a row
        // above it leaves.
        let key = match item.id.as_deref() {
            Some(id) => format!("home-row-{widget}-{id}"),
            None => format!("home-row-{widget}-{ix}"),
        };
        let fade_key = format!("{key}-hover");
        let change = item
            .id
            .as_ref()
            .and_then(|id| state.overrides.get(id))
            .map(|change| change.change);
        let snoozed = change == Some(RowChange::Snoozed) || item.snoozed_until.is_some();
        let link = item
            .link
            .clone()
            .filter(|link| parse_chat_link(link).is_some() || is_openable(link));
        let mark = status_mark(theme, item.status.as_deref(), &key, view, cx);
        let age = item.age.clone().or_else(|| {
            item.at
                .as_deref()
                .and_then(parse_time)
                .map(|at| short_age(at, Utc::now()))
        });
        let heat = item
            .heat
            .map(Heat::from_level)
            .filter(|heat| *heat != Heat::Off);
        let mut right = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(ui_rems(17.0))
            .text_size(ui_rems(11.0))
            .text_color(theme.text_faint)
            .when_some(item.account.clone(), |el, account| {
                el.child(SharedString::from(account))
            });
        // The door sends a loose end's heat word as its badge too; the heat mark says it.
        let badge = item
            .badge
            .clone()
            .filter(|badge| heat.is_none_or(|heat| !badge.eq_ignore_ascii_case(heat.word())));
        if snoozed {
            right = right.child(widgets::badge(theme, "snoozed"));
        } else if let Some(badge) = badge {
            right = right.child(widgets::badge(theme, badge));
        } else if let Some(age) = age {
            right = right.child(
                div()
                    .font_family(theme.font_mono.clone())
                    .child(SharedString::from(age)),
            );
        }
        if let Some(heat) = heat {
            right = right.child(heat_mark(theme, heat, view, cx));
        }
        // Usage rows draw their meters instead of the sub line.
        let sub = item.sub.clone().filter(|_| item.meters.is_empty());
        let text = div()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .truncate()
                    .text_size(ui_rems(13.0))
                    .line_height(ui_rems(17.0))
                    .text_color(theme.text)
                    .child(SharedString::from(item.title.clone())),
            )
            .when_some(sub, |el, sub| {
                el.child(
                    div()
                        .mt(px(1.0))
                        .text_size(ui_rems(11.5))
                        .line_height(ui_rems(15.0))
                        .text_color(theme.text_muted)
                        .line_clamp(2)
                        .text_ellipsis()
                        .child(SharedString::from(sub)),
                )
            })
            .children(super::usage::meters(theme, &item.meters));
        // Actions post to the widget's own source, so only door rows offer them.
        let row_id = item.id.as_deref().filter(|_| {
            !item.actions.is_empty()
                && self.poster.is_some()
                && manifest.source.door_path().is_some()
        });
        let actions = row_id.and_then(|row| {
            let menu_open = self
                .snooze_menu
                .get()
                .is_some_and(|(menu_widget, menu_row)| menu_widget == widget && menu_row == row);
            let focused = self
                .keyboard_row
                .as_ref()
                .is_some_and(|(focus_widget, focus_row)| {
                    focus_widget == widget && focus_row == row
                });
            let (cluster, count) = self
                .render_row_actions(theme, manifest, state, row, item, menu_open, &fade_key, cx);
            (count > 0).then(|| {
                // Hover shows them; so does an open snooze menu or keyboard focus.
                let held = motion::state_t(
                    &format!("{key}-actions"),
                    menu_open || focused,
                    motion::HOVER_FADE,
                    motion::reduced_motion(cx),
                );
                (cluster, count, motion::hover_t(&fade_key).max(held))
            })
        });
        let has_actions = actions.is_some();
        let interactive = link.is_some() || has_actions;
        // The right slot: the row's meta, and its actions fading in over it in
        // the same place.
        let slot = match actions {
            Some((cluster, count, shown)) => div()
                .relative()
                .flex_none()
                .flex()
                .justify_end()
                .h(ui_rems(17.0))
                .min_w(px(actions_width(count)))
                .child(right.opacity(1.0 - shown))
                .child(cluster.opacity(shown)),
            None => right,
        };
        // Where it's drawn whole: a collapse after Done starts from its height.
        let measure = row_id
            .filter(|_| has_actions)
            .map(|row| (state.row_ui.clone(), row.to_string()));
        let probe = interactive.then(|| row_probe(fade_key.clone(), measure));
        let mut row = div()
            .id(SharedString::from(key))
            .relative()
            .flex()
            .items_start()
            .gap(px(8.0))
            .mx(px(-ROW_INSET))
            .px(px(ROW_INSET))
            .py(px(3.0))
            .rounded(px(6.0))
            .when(interactive, |el| {
                let wash = theme.glass_hover();
                el.bg(motion::hover_blend(&fade_key, wash.opacity(0.0), wash))
            })
            .when(snoozed, |el| el.opacity(0.6))
            .children(mark)
            .child(text)
            .child(slot)
            .children(probe);
        if let Some(link) = link {
            let accent = theme.accent;
            row = row
                .cursor_pointer()
                .tab_index(0)
                .role(Role::Link)
                .aria_label(item.title.clone())
                .focus_visible(move |s| s.border_1().border_color(accent))
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.open_link(&link, cx)),
                );
        }
        row.into_any_element()
    }

    /// Snooze (or "not a thing") and Done as quiet icon buttons, Done
    /// rightmost, for the row's right slot. Done on a Gmail or Slack row means
    /// nobody needs a reply. Returns the cluster and how many buttons it has.
    #[allow(clippy::too_many_arguments)]
    fn render_row_actions(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        state: &WidgetState,
        row: &str,
        item: &ListItem,
        menu_open: bool,
        row_fade: &str,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, usize) {
        let widget = manifest.id.as_str();
        let offers = |name: &str| item.actions.iter().any(|action| action == name);
        let pair = (widget.to_string(), row.to_string());
        let focus = state
            .row_ui
            .borrow_mut()
            .entry(row.to_string())
            .or_insert_with(|| RowUi {
                focus: cx.focus_handle(),
                bounds: None,
            })
            .focus
            .clone();
        let tip = |text: &'static str| row_tooltip(text, state.row_ui.clone(), row.to_string());
        let mut buttons: Vec<AnyElement> = Vec::new();
        if offers("snooze") {
            let mut snooze = icon_button(
                theme,
                format!("home-snooze-{widget}-{row}"),
                icons::CLOCK_CIRCLE,
                (!menu_open).then(|| tip("Snooze")),
            )
            .aria_label(format!("Snooze {}", item.title))
            .aria_expanded(menu_open)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let pair = pair.clone();
                    move |this, _, _, _| {
                        this.snooze_menu
                            .note_trigger_press_matching(|open| *open == pair);
                    }
                }),
            )
            .on_click(cx.listener({
                let pair = pair.clone();
                let row_fade = row_fade.to_string();
                move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if !actions_showing(event, &row_fade) {
                        return;
                    }
                    this.open_snooze_menu(&pair.0, &pair.1, cx);
                }
            }));
            if menu_open {
                snooze = snooze.child(self.render_snooze_menu(
                    theme,
                    widget,
                    row,
                    offers("dismiss"),
                    cx,
                ));
            }
            buttons.push(snooze.into_any_element());
        } else if offers("dismiss") {
            buttons.push(
                row_action(
                    theme,
                    &pair,
                    "dismiss",
                    icons::CLOSE,
                    tip("Not a thing"),
                    Action::Dismiss,
                    row_fade,
                    cx,
                )
                .aria_label(format!("{} is not a thing", item.title))
                .into_any_element(),
            );
        }
        if offers("done") {
            let done = if matches!(manifest.source, SourceSpec::KeronSources(_)) {
                row_action(
                    theme,
                    &pair,
                    "done",
                    icons::CHECK,
                    tip("No reply needed"),
                    Action::Done,
                    row_fade,
                    cx,
                )
                .aria_label(format!("Mark {} as needing no reply", item.title))
            } else {
                row_action(
                    theme,
                    &pair,
                    "done",
                    icons::CHECK,
                    tip("Done"),
                    Action::Done,
                    row_fade,
                    cx,
                )
                .aria_label(format!("Mark {} done", item.title))
            };
            buttons.push(done.into_any_element());
        }
        let count = buttons.len();
        let cluster = div()
            .id(SharedString::from(format!("home-actions-{widget}-{row}")))
            .track_focus(&focus)
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .gap(px(ROW_ACTION_GAP))
            .children(buttons);
        (cluster, count)
    }

    fn render_snooze_menu(
        &self,
        theme: &Theme,
        widget: &str,
        row: &str,
        dismiss: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let popup = theme.for_popup();
        let mut menu = popover::popover_card(&popup)
            .w(px(184.0))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_snooze_menu(cx)));
        let item = |key: String, label: SharedString, action: Action| {
            let (widget, row) = (widget.to_string(), row.to_string());
            popover::menu_row(&popup, false, key.clone())
                .id(SharedString::from(key))
                .tab_index(0)
                .role(Role::MenuItem)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.close_snooze_menu(cx);
                    this.act(&widget, &row, action.clone(), cx);
                }))
                .child(label)
        };
        for (ix, (label, until)) in snooze_choices(Local::now()).into_iter().enumerate() {
            menu = menu.child(item(
                format!("home-snooze-{widget}-{row}-{ix}"),
                label.into(),
                Action::Snooze { until },
            ));
        }
        if dismiss {
            menu = menu
                .child(div().my(px(4.0)).h(px(1.0)).bg(popup.border.opacity(0.6)))
                .child(item(
                    format!("home-dismiss-{widget}-{row}"),
                    "Not a thing".into(),
                    Action::Dismiss,
                ));
        }
        popover::anchored_menu_below_end(
            format!("home-snooze-menu-{widget}-{row}"),
            menu.into_any_element(),
            self.snooze_menu.closing_since(),
        )
    }

    // ---- agenda ----

    fn render_agenda(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        items: &[AgendaItem],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if items.is_empty() {
            return empty(theme, manifest);
        }
        let today = Local::now().date_naive();
        let (rows, more) = cap(items, limit(manifest));
        let mut children: Vec<AnyElement> = Vec::new();
        let mut last_label: Option<String> = None;
        for (ix, item) in rows.iter().enumerate() {
            let (date, time) = agenda_when(item);
            let label = date.filter(|date| *date > today).map(|date| {
                if Some(date) == today.succ_opt() {
                    "Tomorrow".to_string()
                } else {
                    date.format("%A").to_string()
                }
            });
            if label != last_label {
                if let Some(label) = &label {
                    children.push(
                        div()
                            .pt(px(4.0))
                            .text_size(ui_rems(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(SharedString::from(label.clone()))
                            .into_any_element(),
                    );
                }
                last_label = label;
            }
            children.push(self.render_agenda_row(theme, manifest, item, ix, time, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(children)
            .children(more_line(theme, more))
            .into_any_element()
    }

    fn render_agenda_row(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        item: &AgendaItem,
        ix: usize,
        time: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("home-event-{}-{ix}", manifest.id);
        let video = item.video_link.clone().filter(|link| is_openable(link));
        let right: Option<AnyElement> = if let Some(video) = video {
            let hover = theme.glass_hover();
            let accent = theme.accent;
            Some(
                div()
                    .id(SharedString::from(format!("{key}-video")))
                    .flex_none()
                    .p(px(2.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .tab_index(0)
                    .role(Role::Button)
                    .aria_label(format!("Join the call for {}", item.title))
                    .focus_visible(move |s| s.border_1().border_color(accent))
                    .tooltip(widgets::text_tooltip("Join the call"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_link(&video, cx);
                    }))
                    .child(
                        icon(icons::VIDEO)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .into_any_element(),
            )
        } else {
            item.attendees.filter(|count| *count > 1).map(|count| {
                div()
                    .flex_none()
                    .text_size(ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!("{count} people")))
                    .into_any_element()
            })
        };
        let link = item.link.clone().filter(|link| is_openable(link));
        let mut row = div()
            .id(SharedString::from(key))
            .flex()
            .items_start()
            .gap(px(8.0))
            .mx(px(-6.0))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(6.0))
            .child(
                div()
                    .flex_none()
                    .w(px(44.0))
                    .text_size(ui_rems(11.0))
                    .line_height(ui_rems(17.0))
                    .font_family(theme.font_mono.clone())
                    .text_color(theme.text_muted)
                    .child(SharedString::from(time)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(6.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(13.0))
                            .line_height(ui_rems(17.0))
                            .text_color(theme.text)
                            .child(SharedString::from(item.title.clone())),
                    )
                    .when_some(item.warn.clone(), |el, warn| {
                        el.child(warn_chip(theme, warn))
                    }),
            )
            .children(right);
        if let Some(link) = link {
            let hover = theme.wash(0.05);
            let accent = theme.accent;
            row = row
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tab_index(0)
                .role(Role::Link)
                .aria_label(item.title.clone())
                .focus_visible(move |s| s.border_1().border_color(accent))
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.open_link(&link, cx)),
                );
        }
        row.into_any_element()
    }
}

// ---- kinds without interaction ----

fn render_timeline(theme: &Theme, manifest: &Manifest, items: &[TimelineItem]) -> AnyElement {
    if items.is_empty() {
        return empty(theme, manifest);
    }
    let (rows, more) = cap(items, limit(manifest));
    let rows = rows.iter().map(|item| {
        let time = item
            .time
            .clone()
            .or_else(|| {
                item.at
                    .as_deref()
                    .and_then(parse_time)
                    .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
            })
            .unwrap_or_default();
        let kind = item.kind.clone().unwrap_or_default();
        div()
            .flex()
            .items_start()
            .gap(px(8.0))
            .child(
                div()
                    .flex_none()
                    .w(px(40.0))
                    .text_size(ui_rems(11.0))
                    .line_height(ui_rems(17.0))
                    .font_family(theme.font_mono.clone())
                    .text_color(theme.text_muted)
                    .child(SharedString::from(time)),
            )
            .child(kind_chip(theme, &kind))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_rems(13.0))
                    .line_height(ui_rems(17.0))
                    .text_color(theme.text)
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(SharedString::from(item.text.clone())),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(rows)
        .children(more_line(theme, more))
        .into_any_element()
}

fn render_devices(theme: &Theme, manifest: &Manifest, items: &[DeviceItem]) -> AnyElement {
    if items.is_empty() {
        return empty(theme, manifest);
    }
    let (rows, more) = cap(items, limit(manifest));
    let rows = rows.iter().map(|item| {
        let mut meta: Vec<String> = Vec::new();
        if let Some(role) = item.role.clone().filter(|role| !role.is_empty()) {
            meta.push(role);
        } else if let Some(platform) = item.platform.as_deref().filter(|p| !p.is_empty()) {
            meta.push(crate::settings::devices::platform_label(platform).to_string());
        }
        if item.is_self {
            meta.push("this Mac".to_string());
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_none()
                    .size(px(7.0))
                    .rounded_full()
                    .bg(if item.online {
                        theme.success
                    } else {
                        theme.text_faint.opacity(0.5)
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(5.0))
                    .text_size(ui_rems(13.0))
                    .line_height(ui_rems(17.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .child(SharedString::from(item.name.clone())),
                    )
                    .when(!meta.is_empty(), |el| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!("· {}", meta.join(" · ")))),
                        )
                    }),
            )
            .when_some(item.detail.clone(), |el, detail| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(ui_rems(11.0))
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.text_faint)
                        .child(SharedString::from(detail)),
                )
            })
    });
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(rows)
        .children(more_line(theme, more))
        .into_any_element()
}

fn render_stat(theme: &Theme, manifest: &Manifest, stat: &Stat) -> AnyElement {
    if stat.value.is_empty() && stat.series.is_empty() {
        return empty(theme, manifest);
    }
    let max = stat.series.iter().copied().fold(0.0_f64, f64::max);
    let last = stat.series.len().saturating_sub(1);
    let bars = stat.series.iter().enumerate().map(|(ix, value)| {
        let share = if max > 0.0 {
            (value / max).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        div()
            .flex_1()
            .h(relative(share.max(0.04)))
            .rounded_t(px(2.0))
            .bg(if ix == last {
                theme.accent
            } else {
                theme.accent.opacity(0.4)
            })
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(8.0))
                .child(
                    div()
                        .text_size(ui_rems(22.0))
                        .line_height(ui_rems(26.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(SharedString::from(stat.value.clone())),
                )
                .when_some(stat.label.clone(), |el, label| {
                    el.child(
                        div()
                            .text_size(ui_rems(12.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(label)),
                    )
                })
                .when_some(stat.delta.clone(), |el, delta| {
                    el.child(
                        div()
                            .text_size(ui_rems(11.5))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(delta)),
                    )
                }),
        )
        .when(!stat.series.is_empty(), |el| {
            el.child(
                div()
                    .h(px(46.0))
                    .flex()
                    .items_end()
                    .gap(px(4.0))
                    .children(bars),
            )
        })
        .into_any_element()
}

/// Problems the source reported, and how old door data is once it's stale.
fn render_footer(
    theme: &Theme,
    manifest: &Manifest,
    state: Option<&WidgetState>,
) -> Option<AnyElement> {
    let payload = state?.payload.as_ref()?;
    let problems = payload.errors.len();
    let stale = freshness(manifest, payload);
    if problems == 0 && stale.is_none() {
        return None;
    }
    let problems_note = if problems > 0 {
        div()
            .id(SharedString::from(format!("home-problems-{}", manifest.id)))
            .text_color(theme.warning_muted)
            .tooltip(widgets::text_tooltip(payload.errors.join("\n")))
            .child(SharedString::from(if problems == 1 {
                "1 problem".to_string()
            } else {
                format!("{problems} problems")
            }))
            .into_any_element()
    } else {
        div().into_any_element()
    };
    Some(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .pt(px(8.0))
            .border_t_1()
            .border_color(widgets::row_divider(theme))
            .text_size(ui_rems(11.5))
            .text_color(theme.text_faint)
            .child(problems_note)
            .children(stale.map(SharedString::from))
            .into_any_element(),
    )
}

/// How old door data is: loose ends' `updated` is the mini's last pass
/// (hourly), not this fetch, so it's always said ("checked 5m ago"); other
/// door sources say it only once it's gone stale.
fn freshness(manifest: &Manifest, payload: &Payload) -> Option<String> {
    let now = Utc::now();
    let pass = matches!(&manifest.source, SourceSpec::Memory(name) if name == "loose-ends");
    payload
        .updated
        .as_deref()
        .filter(|_| is_door(&manifest.source))
        .and_then(parse_time)
        .filter(|at| {
            pass || now
                .signed_duration_since(at.with_timezone(&Utc))
                .to_std()
                .is_ok_and(|age| age > stale_after(manifest))
        })
        .map(|at| {
            let word = if pass { "checked" } else { "updated" };
            format!("{word} {} ago", short_age(at, now))
        })
}

// ---- small pieces ----

/// The card look: the composer pill's fill and edge, so Home reads as part
/// of the same surface.
fn card_surface(theme: &Theme) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(theme.composer_surface_border())
        .bg(theme.composer_surface_bg())
}

/// A focusable settings-style action.
fn button(theme: &Theme, id: &'static str, tone: ActionTone) -> Stateful<Div> {
    let accent = theme.accent;
    widgets::action_button(theme, tone)
        .id(id)
        .flex_none()
        .tab_index(0)
        .role(Role::Button)
        .focus_visible(move |s| s.border_2().border_color(accent))
}

/// The row-sized text button (Retry).
fn small_button(theme: &Theme, id: String) -> Stateful<Div> {
    let accent = theme.accent;
    let hover = theme.glass_hover();
    div()
        .id(SharedString::from(id))
        .flex_none()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_raised)
        .text_size(ui_rems(11.5))
        .text_color(theme.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tab_index(0)
        .role(Role::Button)
        .focus_visible(move |s| s.border_color(accent))
}

/// A row action: a quiet icon-only button, like the app's other small icon
/// actions. Nothing behind it at rest; under the pointer a faint wash, and
/// the glyph goes from muted to the text color. Without a tooltip it's
/// pressed: its menu is open, and it holds that look.
fn icon_button(
    theme: &Theme,
    id: String,
    glyph: &'static str,
    tooltip: Option<impl Fn(&mut Window, &mut App) -> AnyView + 'static>,
) -> Stateful<Div> {
    let pressed = tooltip.is_none();
    let fade_key = format!("{id}-hover");
    let accent = theme.accent;
    let wash = theme.ink(0.08);
    let (bg, ink) = if pressed {
        (wash, theme.text)
    } else {
        (
            motion::hover_blend(&fade_key, wash.opacity(0.0), wash),
            motion::hover_blend(&fade_key, theme.text_muted, theme.text),
        )
    };
    div()
        .id(SharedString::from(id))
        .relative()
        .flex_none()
        .size(px(ROW_ACTION_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(Theme::CONTROL_RADIUS))
        .bg(bg)
        .cursor_pointer()
        .on_hover(motion::hover_listener(fade_key))
        .tab_index(0)
        .role(Role::Button)
        .focus_visible(move |s| s.border_1().border_color(accent))
        .when_some(tooltip, |el, tooltip| el.tooltip(tooltip))
        .child(icon(glyph).size(px(ROW_ACTION_ICON_SIZE)).text_color(ink))
}

fn row_action(
    theme: &Theme,
    pair: &(String, String),
    name: &str,
    glyph: &'static str,
    tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    action: Action,
    row_fade: &str,
    cx: &mut Context<Home>,
) -> Stateful<Div> {
    let (widget, row) = pair.clone();
    let row_fade = row_fade.to_string();
    icon_button(
        theme,
        format!("home-{name}-{widget}-{row}"),
        glyph,
        Some(tooltip),
    )
    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
        cx.stop_propagation();
        // A double click's second press lands on the row that moved up
        // into this one's place.
        if event.click_count() > 1 || !actions_showing(event, &row_fade) {
            return;
        }
        this.act(&widget, &row, action.clone(), cx);
    }))
}

/// Whether a click on a row's action counts: from the keyboard, or with
/// the actions shown. Hidden, they're still under the pointer.
fn actions_showing(event: &ClickEvent, row_fade: &str) -> bool {
    event.is_keyboard() || motion::hover_t(row_fade) > 0.0
}

thread_local! {
    /// The rows hovered as of their last frame, by hover-fade key.
    static HOVERED_ROWS: RefCell<HashSet<String>> = RefCell::default();
}

/// Brings the hover fade behind `key` in line with `hovered`; true when it
/// changed, and the row needs another frame to show it.
fn sync_row_hover(key: &str, hovered: bool, reduced: bool) -> bool {
    HOVERED_ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        let noted = rows.contains(key);
        // The fade store forgets a row that went a frame undrawn.
        let current = noted && motion::hover_t(key) > 0.0;
        if hovered == current {
            if noted && !hovered {
                rows.remove(key);
            }
            return false;
        }
        if hovered {
            rows.insert(key.to_string());
        } else {
            rows.remove(key);
        }
        motion::set_hover(key, hovered, reduced);
        true
    })
}

/// Keeps a row's hover in step with what's under the pointer every frame,
/// as gpui's own hover styles do. Pointer events alone miss a row moving
/// under a still pointer: one above it closed up, the card scrolled, or a
/// refetch added a row. Also notes where a row with actions was drawn.
fn row_probe(
    fade_key: String,
    measure: Option<(Rc<RefCell<HashMap<String, RowUi>>>, String)>,
) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            if let Some((rows, row)) = measure
                && let Some(ui) = rows.borrow_mut().get_mut(&row)
            {
                ui.bounds = Some(bounds);
            }
            window.insert_hitbox(bounds, HitboxBehavior::Normal)
        },
        move |_, hitbox, window, cx| {
            let hovered = hitbox.is_hovered(window);
            if sync_row_hover(&fade_key, hovered, motion::reduced_motion(cx)) {
                window.request_animation_frame();
            }
            let view = window.current_view();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && hitbox.is_hovered(window) != hovered {
                    cx.notify(view);
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

/// The slot a row's actions need: buttons and the gaps between them.
fn actions_width(count: usize) -> f32 {
    let count = count as f32;
    count * ROW_ACTION_SIZE + (count - 1.0).max(0.0) * ROW_ACTION_GAP
}

/// A row on its way out after Done or "not a thing": it fades as its height
/// closes, taking the gap under it along, so the rows below move up without
/// a jump.
fn collapsing(leaving: &Leaving, row: AnyElement) -> AnyElement {
    let t = leaving.progress();
    div()
        .flex_none()
        // Room for the row's hover wash, which reaches past the text column.
        .mx(px(-ROW_INSET))
        .px(px(ROW_INSET))
        .overflow_hidden()
        .h(px(leaving.height * (1.0 - t)))
        .mb(px(-LIST_GAP * t))
        .opacity(1.0 - t)
        .child(row)
        .into_any_element()
}

/// A row joining a list as one above it closes after Done: it opens and
/// fades in by the same eased amount, bringing its gap along, so the card
/// keeps its height.
fn joining(leaving: &Leaving, height: f32, row: AnyElement) -> AnyElement {
    let t = leaving.progress();
    div()
        .flex_none()
        .mx(px(-ROW_INSET))
        .px(px(ROW_INSET))
        .overflow_hidden()
        .h(px(height * t))
        .mt(px(-LIST_GAP * (1.0 - t)))
        .opacity(t)
        .child(row)
        .into_any_element()
}

/// A row action's tooltip: the app's usual chip, just above the row and
/// ending at the pointer. gpui hangs a tooltip below and right of the
/// pointer, which by a card's right edge spills onto the next card.
struct RowTooltip {
    chip: AnyView,
    /// From the pointer up to the chip's bottom edge.
    lift: f32,
}

impl Render for RowTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_0().child(
            div()
                .absolute()
                .right_0()
                .bottom(px(self.lift))
                .whitespace_nowrap()
                .child(self.chip.clone()),
        )
    }
}

/// `.tooltip(...)` for a row action, placed from where `row` was last drawn.
fn row_tooltip(
    text: &'static str,
    rows: Rc<RefCell<HashMap<String, RowUi>>>,
    row: String,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let chip = widgets::text_tooltip(text);
    move |window, cx| {
        let pointer = window.mouse_position();
        let lift = rows.borrow().get(&row).and_then(|ui| ui.bounds).map_or(
            ROW_TOOLTIP_FALLBACK_LIFT,
            |bounds| {
                // gpui hangs the tooltip 1px off the pointer.
                f32::from(pointer.y - bounds.top()) + 1.0 + ROW_TOOLTIP_GAP
            },
        );
        let chip = chip(window, cx);
        cx.new(|_| RowTooltip { chip, lift }).into()
    }
}

fn skeleton(theme: &Theme, view: EntityId, cx: &mut App) -> AnyElement {
    popover::skeleton_rows("home-skeleton", theme, 3, view, cx)
}

fn muted(theme: &Theme, text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(ui_rems(12.5))
        .line_height(ui_rems(17.0))
        .text_color(theme.text_muted)
        .child(text.into())
        .into_any_element()
}

fn empty(theme: &Theme, manifest: &Manifest) -> AnyElement {
    muted(theme, empty_text(manifest))
}

/// What a widget says when it has nothing to show.
fn empty_text(manifest: &Manifest) -> String {
    manifest
        .empty
        .clone()
        .unwrap_or_else(|| "Nothing here".to_string())
}

fn more_line(theme: &Theme, more: usize) -> Option<Div> {
    (more > 0).then(|| {
        div()
            .text_size(ui_rems(11.5))
            .line_height(ui_rems(MORE_LINE_HEIGHT))
            .text_color(theme.text_faint)
            .child(SharedString::from(format!("+{more} more")))
    })
}

fn warn_chip(theme: &Theme, text: String) -> Div {
    div()
        .flex_none()
        .px(px(5.0))
        .rounded(px(4.0))
        .bg(theme.warning.opacity(0.14))
        .text_size(ui_rems(10.5))
        .line_height(ui_rems(15.0))
        .text_color(theme.warning)
        .child(SharedString::from(text))
}

/// A session's state at the row's start: working spins, waiting is an amber
/// dot, an error a red one.
fn status_mark(
    theme: &Theme,
    status: Option<&str>,
    key: &str,
    view: EntityId,
    cx: &mut App,
) -> Option<AnyElement> {
    let dot = |color: Hsla| {
        div()
            .size(px(7.0))
            .rounded_full()
            .bg(color)
            .into_any_element()
    };
    let mark = match status? {
        "working" => {
            crate::loaders::mini_glyph_spinner(format!("{key}-working"), 2.0, theme.glyph, view, cx)
                .into_any_element()
        }
        "waiting" => dot(theme.warning),
        "error" => dot(theme.danger),
        _ => return None,
    };
    Some(
        div()
            .flex_none()
            .w(px(10.0))
            .h(ui_rems(17.0))
            .flex()
            .items_center()
            .justify_center()
            .child(mark)
            .into_any_element(),
    )
}

/// Four rising bars in one hue and the heat's word. Burning breathes,
/// except under Reduce Motion.
fn heat_mark(theme: &Theme, heat: Heat, view: EntityId, cx: &mut App) -> Div {
    let color = heat_color(theme, heat);
    let unlit = theme.text.opacity(0.14);
    let lit = heat.bars();
    let opacity = if heat.pulses() {
        1.0 - 0.5 * motion::pulse_wave(motion::pulse_delta_slow(&HEAT_PULSE, view, cx))
    } else {
        1.0
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(2.0))
                .opacity(opacity)
                .children(
                    [6.0, 8.0, 10.0, 12.0]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, height)| {
                            div()
                                .w(px(4.0))
                                .h(px(height))
                                .rounded(px(1.0))
                                .bg(if (ix as u8) < lit { color } else { unlit })
                        }),
                ),
        )
        .child(
            div()
                .min_w(px(44.0))
                .text_size(ui_rems(10.5))
                .text_color(color)
                .child(heat.word()),
        )
}

fn heat_color(theme: &Theme, heat: Heat) -> Hsla {
    match heat {
        Heat::Off => theme.text_faint,
        Heat::Quiet => crate::theme::mix(theme.text_muted, preset(theme, AccentPreset::Blue), 0.35),
        Heat::Warm => theme.warning,
        Heat::Hot => preset(theme, AccentPreset::Orange),
        Heat::Burning => theme.danger,
    }
}

/// A timeline row's kind: did, said, heard, agent and nudge each get their
/// own quiet hue.
fn kind_chip(theme: &Theme, kind: &str) -> Div {
    let chip = div().flex_none().w(px(52.0));
    if kind.is_empty() {
        return chip;
    }
    let hue = match kind {
        "did" => preset(theme, AccentPreset::Blue),
        "said" => preset(theme, AccentPreset::Orange),
        "heard" => preset(theme, AccentPreset::Amber),
        "agent" => preset(theme, AccentPreset::Green),
        "nudge" => preset(theme, AccentPreset::Zeron),
        _ => theme.text_muted,
    };
    chip.mt(px(1.0))
        .rounded(px(4.0))
        .bg(hue.opacity(0.14))
        .flex()
        .justify_center()
        .text_size(ui_rems(10.5))
        .line_height(ui_rems(15.0))
        .text_color(hue)
        .child(SharedString::from(kind.to_string()))
}

/// An accent preset's color for the current appearance.
fn preset(theme: &Theme, preset: AccentPreset) -> Hsla {
    let appearance = if theme.appearance.is_dark() {
        zeron_theme::Appearance::Dark
    } else {
        zeron_theme::Appearance::Light
    };
    let color = preset.color(appearance);
    gpui::rgba(u32::from_be_bytes([color.r, color.g, color.b, color.a])).into()
}

fn icon_for(manifest: &Manifest) -> &'static str {
    match manifest.icon.as_deref() {
        Some("flag") => icons::FLAG,
        Some("chat") => icons::CHAT_ROUND_LINE,
        Some("check") => icons::CHECK,
        Some("calendar") => icons::CALENDAR,
        Some("tree") => icons::FILE_TREE,
        Some("pulse") => icons::PULSE,
        Some("devices") => icons::MONITOR,
        Some("pr") => icons::PULL_REQUEST,
        Some("mail") => icons::MAIL,
        Some("mic") => icons::MICROPHONE,
        Some("bars") => icons::BARS,
        Some("widget") => icons::WIDGET,
        Some("bell") => icons::BELL,
        Some("star") => icons::STAR,
        Some("list") => icons::LIST,
        Some("globe") => icons::GLOBE,
        _ => match manifest.kind {
            Kind::List => icons::LIST,
            Kind::Timeline => icons::FILE_TREE,
            Kind::Stat => icons::BARS,
            Kind::Agenda => icons::CALENDAR,
            Kind::Devices => icons::MONITOR,
        },
    }
}

/// The card's count, and whether anything in it is hot or burning.
fn card_count(state: Option<&WidgetState>) -> (Option<usize>, bool) {
    let Some(state) = state else {
        return (None, false);
    };
    let Some(payload) = &state.payload else {
        return (None, false);
    };
    match &payload.body {
        Body::List(items) => {
            let (rows, _) = visible_list_rows(items, &state.overrides, usize::MAX);
            let hot = rows.iter().any(|row| {
                row.heat
                    .is_some_and(|heat| Heat::from_level(heat) >= Heat::Hot)
            });
            (Some(rows.len()), hot)
        }
        Body::Stat(_) => (None, false),
        body => (Some(body.len()), false),
    }
}

/// Door data counts as stale after two missed refreshes (two minutes at
/// least).
fn stale_after(manifest: &Manifest) -> Duration {
    manifest
        .every
        .saturating_mul(2)
        .max(Duration::from_secs(120))
}

/// When an event is: its local date, and "10:00" or "All day".
fn agenda_when(item: &AgendaItem) -> (Option<NaiveDate>, String) {
    if !item.all_day
        && let Some(at) = parse_time(&item.start)
    {
        let local = at.with_timezone(&Local);
        return (Some(local.date_naive()), local.format("%H:%M").to_string());
    }
    let date = parse_date(&item.start)
        .or_else(|| parse_time(&item.start).map(|at| at.with_timezone(&Local).date_naive()));
    (date, "All day".to_string())
}

fn cap<T>(items: &[T], limit: usize) -> (&[T], usize) {
    let shown = items.len().min(limit);
    (&items[..shown], items.len() - shown)
}

/// `0..len` with the item at `from` moved to `to`.
fn moved(len: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if from < len {
        let item = order.remove(from);
        order.insert(to.min(len - 1), item);
    }
    order
}

fn toggled(on: bool) -> Toggled {
    if on { Toggled::True } else { Toggled::False }
}
