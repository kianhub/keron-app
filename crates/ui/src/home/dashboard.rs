//! The Mini dashboard (`kind = "dashboard"`, from `memory:dashboard`): a wide
//! card on Home and the page it opens into, a Grafana-like board of what the
//! Mac mini processed. The figures arrive formatted
//! ([`keron_home::dashboard`]); this lays them out.
//!
//! - The card: a few tiles and one chart of the card's period, and the
//!   year's heatmap. A click on it, or its "Open dashboard", opens the page.
//! - The page, in the grid's place: Back, the period switch (Day, Week,
//!   Month, Year), every tile of the period, its charts as panels two across,
//!   its breakdown tables, the heatmap with its month and weekday labels, and
//!   the mini's footnote.
//!
//! Bars are divs, each with its own tooltip. The heatmap is one canvas: the
//! pointer is hit-tested against its days, and the day's tooltip is drawn
//! above its cell. Each chart and the heatmap read to a screen reader as one
//! image with a summary. A series takes its color from its id ([`slot`]),
//! never its rank, so a job keeps its color across charts and periods.

use gpui::{
    Anchor, AnchoredPositionMode, AnyElement, App, BorderStyle, Bounds, ClickEvent, Context,
    DispatchPhase, Div, FocusHandle, FontWeight, Hitbox, HitboxBehavior, Hsla, KeyDownEvent,
    MouseMoveEvent, Pixels, Point, Role, SharedString, Stateful, Window, div, point, prelude::*,
    px, relative, size,
};
use keron_home::Payload;
use keron_home::dashboard::{
    Breakdown, Chart, Dashboard, Day, Heatmap, Tile, Tone, WEEKS, scale_text,
};

use super::view::{button, card_surface, empty, toggled};
use super::{Home, Manifest, SignIn, Slot, is_door};
use crate::icons::{self, icon};
use crate::settings::widgets::{self, ActionTone};
use crate::theme::{CHART_SLOTS, Theme};
use crate::typography::ui_rems;

/// The dashboard page while it's open.
pub(super) struct Page {
    pub(super) widget: String,
    /// The page's own focus, where Escape closes it.
    pub(super) focus: FocusHandle,
}

/// The heatmap day under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HeatHover {
    /// Which heatmap: the card's or the page's.
    pub(super) key: String,
    pub(super) day: usize,
    /// The middle of its cell's top edge, from the heatmap's top left.
    pub(super) at: Point<Pixels>,
}

/// The page starts under the window's titlebar.
pub(super) const PAGE_TOP: f32 = Theme::TITLEBAR_HEIGHT + Theme::SPACE_SM;
const CARD_CHART_HEIGHT: f32 = 44.0;
const PANEL_CHART_HEIGHT: f32 = 120.0;
/// The page lays its panels two across from this wide.
const TWO_PANELS_FROM: f32 = 720.0;
const PANEL_GAP: f32 = 10.0;
const PANEL_PAD: f32 = 12.0;
/// The page's heatmap grows with the page up to this many pixels a day.
const MAX_PITCH: f32 = 16.0;
/// Weekday labels show beside the heatmap once a day is this tall.
const WEEKDAY_LABELS_FROM: f32 = 10.0;
const WEEKDAY_WIDTH: f32 = 28.0;
const MONTH_ROW: f32 = 14.0;
/// At most this many labels under a chart's bars.
const MAX_X_LABELS: usize = 8;
/// A breakdown cell's width.
const CELL_WIDTH: f32 = 64.0;

/// A series' color slot, fixed by what it is: the jobs, the door's apps and
/// the poll series each in their own order (a chart shows only one of
/// these sets, so neighbours in it stay apart), Clef's neurons on violet,
/// and anything else by a hash of its id.
pub(super) fn slot(id: &str) -> usize {
    const FIXED: [(&str, usize); 19] = [
        ("loose-ends", 0),
        ("brief", 1),
        ("naps", 2),
        ("judge", 3),
        ("screen-notes", 4),
        ("compare", 5),
        ("checks", 6),
        ("backup", 7),
        ("claude-code", 0),
        ("codex", 1),
        ("claude-ai", 2),
        ("cursor", 3),
        ("chatgpt", 4),
        ("keron-app", 5),
        ("capture", 6),
        ("other", 7),
        ("rounds", 0),
        ("failures", 7),
        ("neurons", 6),
    ];
    FIXED.iter().find(|(name, _)| *name == id).map_or_else(
        || {
            // FNV-1a: the same slot in every build.
            let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
            (hash % CHART_SLOTS as u64) as usize
        },
        |(_, slot)| *slot,
    )
}

/// A heatmap level's color: the text's faint wash for nothing, then the
/// accent, stronger at each level.
fn level_colors(theme: &Theme) -> [Hsla; 5] {
    [
        theme.text.opacity(0.08),
        theme.accent.opacity(0.3),
        theme.accent.opacity(0.5),
        theme.accent.opacity(0.75),
        theme.accent,
    ]
}

impl Home {
    // ---- the card ----

    /// The card's body: its tiles, its chart and the year's heatmap.
    pub(super) fn render_dashboard_card(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        payload: &Payload,
        dashboard: &Dashboard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if dashboard.is_empty() {
            return empty(theme, manifest);
        }
        let tiles = dashboard.card_tiles();
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .when(!tiles.is_empty(), |el| {
                el.child(tile_row(theme, &tiles, false, "home-dashboard-tile"))
            })
            .children(
                dashboard
                    .card_chart()
                    .map(|chart| card_chart(theme, chart, &manifest.id)),
            )
            .children(dashboard.heatmap.as_ref().map(|heatmap| {
                self.heatmap(theme, heatmap, format!("card-{}", manifest.id), None, cx)
            }))
            .children(problems(theme, &payload.errors))
            .into_any_element()
    }

    /// The card's "Open dashboard", at the right of its header.
    pub(super) fn open_dashboard_button(
        &self,
        theme: &Theme,
        manifest: &Manifest,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = manifest.id.clone();
        let accent = theme.accent;
        let wash = theme.ink(0.08);
        let text = theme.text;
        div()
            .id("home-dashboard-open")
            .debug_selector(|| "home-dashboard-open".to_string())
            .flex_none()
            .flex()
            .items_center()
            .gap(px(3.0))
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(Theme::CONTROL_RADIUS))
            .text_size(ui_rems(11.5))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(move |s| s.bg(wash).text_color(text))
            .tab_index(0)
            .role(Role::Button)
            .aria_label(format!("Open the {} dashboard", manifest.title))
            .focus_visible(move |s| s.border_1().border_color(accent))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                // The card would open it too.
                cx.stop_propagation();
                this.open_dashboard(&id, window, cx);
            }))
            .child("Open dashboard")
            .child(
                icon(icons::ALT_ARROW_RIGHT)
                    .size(px(12.0))
                    .text_color(theme.text_faint),
            )
            .into_any_element()
    }

    // ---- the page ----

    /// The page of `slot`'s dashboard, `width` wide.
    pub(super) fn render_dashboard_page(
        &self,
        theme: &Theme,
        slot: &Slot,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let manifest = &slot.manifest;
        let state = self.widgets.get(&manifest.id);
        let ready = !is_door(&manifest.source) || self.sign_in == SignIn::SignedIn;
        let shown = state
            .and_then(|state| state.payload.as_ref())
            .filter(|_| ready)
            .and_then(|payload| match &payload.body {
                keron_home::Body::Dashboard(dashboard) => Some((payload, dashboard.as_ref())),
                _ => None,
            });
        let period = shown.and_then(|(_, dashboard)| dashboard.period_or_first(self.period));
        let range = period.and_then(|(_, data)| data.range.clone());
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(10.0))
            .gap_y(px(6.0))
            .child(
                button(theme, "home-dashboard-back", ActionTone::Quiet)
                    .debug_selector(|| "home-dashboard-back".to_string())
                    .aria_label("Back to Home")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_dashboard(cx)))
                    .child(
                        icon(icons::ALT_ARROW_LEFT)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    )
                    .child("Home"),
            )
            .child(
                div()
                    .text_size(ui_rems(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from(format!("{} dashboard", manifest.title))),
            )
            .children(range.map(|range| {
                div()
                    .text_size(ui_rems(12.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(range))
            }))
            .child(div().flex_1())
            .children(shown.zip(period).map(|((_, dashboard), (period, _))| {
                self.period_switch(theme, dashboard, period, cx)
            }));
        let body =
            match shown {
                Some((payload, dashboard)) => {
                    // A heatmap with no period still draws.
                    let data = period.map(|(_, data)| data);
                    let two = width >= TWO_PANELS_FROM;
                    let charts: Vec<AnyElement> = data
                        .into_iter()
                        .flat_map(|data| &data.charts)
                        .enumerate()
                        .map(|(ix, chart)| chart_panel(theme, chart, ix))
                        .collect();
                    let tables: Vec<AnyElement> = data
                        .into_iter()
                        .flat_map(|data| &data.breakdowns)
                        .enumerate()
                        .map(|(ix, table)| breakdown_panel(theme, table, ix))
                        .collect();
                    let tiles: Vec<&Tile> = data.into_iter().flat_map(|data| &data.tiles).collect();
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(PANEL_GAP))
                        .children(
                            state
                                .and_then(|state| state.error.as_ref())
                                .map(|message| self.render_error(theme, manifest, message, cx)),
                        )
                        .children(problems(theme, &payload.errors))
                        .when(!tiles.is_empty(), |el| {
                            el.child(tile_row(theme, &tiles, true, "home-dashboard-page-tile"))
                        })
                        .children(panel_grid(charts, two))
                        .children(panel_grid(tables, two))
                        .children(dashboard.heatmap.as_ref().map(|heatmap| {
                            self.heatmap_panel(theme, heatmap, &manifest.id, width, cx)
                        }))
                        .children(dashboard.note.clone().map(|note| {
                            div()
                                .text_size(ui_rems(11.5))
                                .line_height(ui_rems(16.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(note))
                        }))
                        .into_any_element()
                }
                // Signed out, loading or failed: as the card says it.
                None => self.render_body(theme, manifest, state, cx),
            };
        let focus = self.page.as_ref().map(|page| page.focus.clone());
        div()
            .id("home-dashboard-page")
            .debug_selector(|| "home-dashboard-page".to_string())
            .when_some(focus, |el, focus| el.track_focus(&focus))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_dashboard(cx);
                }
            }))
            .w(px(width))
            .mx_auto()
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(header)
            .children(self.render_sign_in(theme, cx))
            .child(body)
            .into_any_element()
    }

    /// Day, Week, Month and Year, those the mini sent, as radio buttons.
    fn period_switch(
        &self,
        theme: &Theme,
        dashboard: &Dashboard,
        shown: keron_home::Period,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        div()
            .id("home-dashboard-periods")
            .role(Role::RadioGroup)
            .aria_label("Period")
            .flex()
            .flex_none()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(7.0))
            .bg(theme.wash(0.05))
            .children(dashboard.periods.iter().map(|(period, _)| {
                let period = *period;
                let selected = period == shown;
                div()
                    .id(SharedString::from(format!(
                        "home-dashboard-period-{}",
                        period.name()
                    )))
                    .debug_selector(move || format!("home-dashboard-period-{}", period.name()))
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .text_size(ui_rems(12.0))
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .when(selected, |el| el.bg(theme.wash(0.1)))
                    .cursor_pointer()
                    .tab_index(0)
                    .role(Role::RadioButton)
                    .aria_label(period.title())
                    .aria_toggled(toggled(selected))
                    .focus_visible(move |s| s.border_1().border_color(accent))
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.set_period(period, cx)),
                    )
                    .child(period.title())
            }))
            .into_any_element()
    }

    /// The page's heatmap: month labels over its weeks, Mon, Wed and Fri
    /// beside them, and a "Less … More" legend.
    fn heatmap_panel(
        &self,
        theme: &Theme,
        heatmap: &Heatmap,
        widget: &str,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let room = width - 2.0 * PANEL_PAD - 2.0 - WEEKDAY_WIDTH - 6.0;
        let pitch = (room / WEEKS as f32).clamp(1.0, MAX_PITCH);
        let label = |text: &str| {
            div()
                .text_size(ui_rems(10.5))
                .line_height(px(MONTH_ROW))
                .text_color(theme.text_faint)
                .child(SharedString::from(text.to_string()))
        };
        let weekdays = div()
            .flex_none()
            .w(px(WEEKDAY_WIDTH))
            .pt(px(MONTH_ROW + 4.0))
            .when(pitch >= WEEKDAY_LABELS_FROM, |el| {
                el.children((0..7).map(|row| {
                    let name = match row {
                        0 => "Mon",
                        2 => "Wed",
                        4 => "Fri",
                        _ => "",
                    };
                    div().h(px(pitch)).flex().items_center().child(label(name))
                }))
            });
        let months = div()
            .relative()
            .w(px(pitch * WEEKS as f32))
            .h(px(MONTH_ROW))
            .children(heatmap.months.iter().map(|(column, name)| {
                div()
                    .absolute()
                    .top_0()
                    .left(px(*column as f32 * pitch))
                    .child(label(name))
            }));
        let colors = level_colors(theme);
        let legend = div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .child(label("Less"))
            .children(
                colors
                    .iter()
                    .map(|color| div().size(px(10.0)).rounded(px(2.0)).bg(*color)),
            )
            .child(label("More"));
        let panel = card_surface(theme)
            .id("home-dashboard-heatmap-panel")
            .px(px(PANEL_PAD))
            .py(px(10.0))
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(panel_title(theme, &heatmap.label))
                    .child(div().flex_1())
                    .child(legend),
            )
            .child(
                div().flex().gap(px(6.0)).child(weekdays).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(months)
                        .child(self.heatmap(
                            theme,
                            heatmap,
                            format!("page-{widget}"),
                            Some(pitch),
                            cx,
                        )),
                ),
            );
        frosted(panel)
    }

    /// The year of days as one canvas, `pitch` pixels a day, or filling the
    /// width when `None`. The day under the pointer gets a ring and its
    /// tooltip above it.
    fn heatmap(
        &self,
        theme: &Theme,
        heatmap: &Heatmap,
        key: String,
        pitch: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = level_colors(theme);
        let levels: Vec<u8> = heatmap.days.iter().map(|day| day.level).collect();
        let hovered = self
            .heat_hover
            .as_ref()
            .filter(|hover| hover.key == key && hover.day < levels.len())
            .cloned();
        let hovered_day = hovered.as_ref().map(|hover| hover.day);
        let ring = theme.text_muted;
        let home = cx.entity().downgrade();
        let canvas = gpui::canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            {
                let key = key.clone();
                move |bounds: Bounds<Pixels>, hitbox: Hitbox, window: &mut Window, _: &mut App| {
                    let pitch = f32::from(bounds.size.width) / WEEKS as f32;
                    let (gap, radius) = cell_style(pitch);
                    let cell = |ix: usize| {
                        Bounds::new(
                            bounds.origin + cell_origin(ix, pitch),
                            size(px(pitch - gap), px(pitch - gap)),
                        )
                    };
                    for (ix, level) in levels.iter().enumerate() {
                        let color = colors[usize::from(*level).min(4)];
                        window.paint_quad(gpui::fill(cell(ix), color).corner_radii(px(radius)));
                    }
                    if let Some(day) = hovered_day {
                        window.paint_quad(
                            gpui::outline(cell(day), ring, BorderStyle::Solid)
                                .corner_radii(px(radius)),
                        );
                    }
                    let count = levels.len();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let day = hitbox
                            .is_hovered(window)
                            .then(|| day_at(event.position - bounds.origin, pitch, count))
                            .flatten();
                        if day == hovered_day {
                            return;
                        }
                        let hover = day.map(|day| HeatHover {
                            key: key.clone(),
                            day,
                            at: cell_origin(day, pitch) + point(px((pitch - gap) / 2.0), px(0.0)),
                        });
                        home.update(cx, |home, cx| home.hover_heat(&key, hover, cx))
                            .ok();
                    });
                }
            },
        );
        let canvas = match pitch {
            Some(pitch) => canvas.w(px(pitch * WEEKS as f32)).h(px(pitch * 7.0)),
            None => canvas.w_full().aspect_ratio(WEEKS as f32 / 7.0),
        };
        let tip = hovered.map(|hover| {
            let text = day_text(&heatmap.days[hover.day]);
            div().absolute().top_0().left_0().size_0().child(
                gpui::deferred(
                    gpui::anchored()
                        .position_mode(AnchoredPositionMode::Local)
                        .position(hover.at - point(px(0.0), px(4.0)))
                        .anchor(Anchor::BottomCenter)
                        .snap_to_window_with_margin(px(8.0))
                        .child(
                            div()
                                .debug_selector(|| "home-dashboard-heat-tip".to_string())
                                .whitespace_nowrap()
                                .child(widgets::tooltip_chip(theme, text.into())),
                        ),
                )
                .priority(2),
            )
        });
        let selector = format!("home-dashboard-heatmap-{key}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .relative()
            .when(pitch.is_none(), |el| el.w_full())
            .role(Role::Image)
            .aria_label(heatmap_summary(heatmap))
            .child(canvas)
            .children(tip)
            .into_any_element()
    }
}

/// A day's gap to the next and its corner radius, at `pitch` pixels a day.
fn cell_style(pitch: f32) -> (f32, f32) {
    ((pitch * 0.18).clamp(1.0, 3.0), (pitch * 0.2).min(2.0))
}

/// Day `ix`'s cell from the heatmap's top left: week columns, Monday on top.
fn cell_origin(ix: usize, pitch: f32) -> Point<Pixels> {
    point(px((ix / 7) as f32 * pitch), px((ix % 7) as f32 * pitch))
}

/// The day under `at` (from the heatmap's top left), if there's one.
fn day_at(at: Point<Pixels>, pitch: f32, count: usize) -> Option<usize> {
    let (x, y) = (f32::from(at.x), f32::from(at.y));
    if x < 0.0 || y < 0.0 || pitch <= 0.0 {
        return None;
    }
    let (column, row) = ((x / pitch) as usize, (y / pitch) as usize);
    let ix = column * 7 + row;
    (column < WEEKS && row < 7 && ix < count).then_some(ix)
}

/// A day's tooltip: the mini's, or its date when it sent none.
fn day_text(day: &Day) -> String {
    day.text
        .clone()
        .unwrap_or_else(|| format!("{} · nothing", day.date.format("%a %-d %b")))
}

/// What the heatmap says to a screen reader.
fn heatmap_summary(heatmap: &Heatmap) -> String {
    let mut summary = format!(
        "{}, the last {WEEKS} weeks: {} days with anything",
        heatmap.label,
        heatmap.active_days()
    );
    if let Some(busiest) = heatmap.busiest() {
        summary.push_str(&format!("; the busiest, {}", day_text(busiest)));
    }
    summary
}

/// What a chart says to a screen reader: its title and its tallest bar.
fn chart_summary(chart: &Chart) -> String {
    let totals = chart.totals();
    let tallest = totals
        .iter()
        .enumerate()
        .filter(|(_, total)| **total > 0.0)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(ix, _)| bar_text(chart, ix));
    match tallest {
        Some(text) => format!(
            "{}: {} bars; the tallest, {text}",
            chart.title,
            chart.labels.len()
        ),
        None => format!("{}: nothing yet", chart.title),
    }
}

/// A bar's tooltip: the mini's, or its label.
fn bar_text(chart: &Chart, ix: usize) -> String {
    Some(&chart.texts[ix])
        .filter(|text| !text.is_empty())
        .unwrap_or(&chart.labels[ix])
        .clone()
}

/// Tiles in rows that wrap, each at least a compact or a large tile wide.
fn tile_row(theme: &Theme, tiles: &[&Tile], large: bool, selector: &'static str) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(8.0))
        .children(tiles.iter().map(|tile| {
            let id = tile.id.clone();
            let delta = match tile.tone {
                Tone::Up | Tone::Down => theme.text_muted,
                Tone::Flat => theme.text_faint,
            };
            div()
                .debug_selector(move || format!("{selector}-{id}"))
                .flex_1()
                .min_w(px(if large { 150.0 } else { 104.0 }))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(theme.wash(0.04))
                .child(
                    div()
                        .truncate()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(tile.label.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(ui_rems(if large { 22.0 } else { 17.0 }))
                                .line_height(ui_rems(if large { 28.0 } else { 22.0 }))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(SharedString::from(tile.text.clone())),
                        )
                        .children(tile.delta.clone().map(|text| {
                            div()
                                .text_size(ui_rems(11.0))
                                .text_color(delta)
                                .child(SharedString::from(text))
                        })),
                )
                .children(tile.sub.clone().map(|sub| {
                    div()
                        .truncate()
                        .text_size(ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(sub))
                }))
        }))
}

/// The card's chart: its bars on a baseline, its first and last labels.
fn card_chart(theme: &Theme, chart: &Chart, widget: &str) -> AnyElement {
    let label = |text: &str| {
        div()
            .text_size(ui_rems(10.5))
            .font_family(theme.font_mono.clone())
            .text_color(theme.text_faint)
            .child(SharedString::from(text.to_string()))
    };
    div()
        .id(SharedString::from(format!("home-dashboard-chart-{widget}")))
        .role(Role::Image)
        .aria_label(chart_summary(chart))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(bars(theme, chart, CARD_CHART_HEIGHT))
        .child(baseline(theme))
        .child(
            div()
                .flex()
                .justify_between()
                .children(chart.labels.first().map(|text| label(text)))
                .children(
                    chart
                        .labels
                        .last()
                        .filter(|_| chart.labels.len() > 1)
                        .map(|text| label(text)),
                ),
        )
        .into_any_element()
}

/// One panel per chart: its title and unit, a legend for two series or
/// more, its scale, its bars, its threshold dashed with its label, and its
/// labels thinned to [`MAX_X_LABELS`].
fn chart_panel(theme: &Theme, chart: &Chart, ix: usize) -> AnyElement {
    let top = chart.scale();
    let faint = |text: String| {
        div()
            .text_size(ui_rems(10.5))
            .font_family(theme.font_mono.clone())
            .text_color(theme.text_faint)
            .child(SharedString::from(text))
    };
    let legend = (chart.series.len() > 1).then(|| {
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(10.0))
            .gap_y(px(2.0))
            .children(chart.series.iter().map(|series| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded(px(2.0))
                            .bg(theme.chart_series(slot(&series.id))),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(series.label.clone())),
                    )
            }))
    });
    let threshold = chart.line.as_ref().filter(|_| top > 0.0).map(|line| {
        let y = PANEL_CHART_HEIGHT * (line.value / top) as f32;
        let color = theme.text_muted.opacity(0.7);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(y))
            .border_t_1()
            .border_dashed()
            .border_color(color)
            .children(line.label.clone().map(|label| {
                div()
                    .absolute()
                    .right_0()
                    .bottom(px(2.0))
                    .text_size(ui_rems(10.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(label))
            }))
    });
    let step = chart.labels.len().div_ceil(MAX_X_LABELS).max(1);
    let gap = bar_gap(chart);
    let x_labels = div()
        .flex()
        .gap(px(gap))
        .children(chart.labels.iter().enumerate().map(|(ix, text)| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .overflow_hidden()
                .when(ix % step == 0, |el| el.child(faint(text.clone())))
        }));
    let panel = card_surface(theme)
        .id(("home-dashboard-panel", ix))
        .px(px(PANEL_PAD))
        .py(px(10.0))
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(panel_title(theme, &chart.title))
                .child(div().flex_1())
                .children(chart.unit.clone().map(faint)),
        )
        .children(legend)
        .child(
            div()
                .id(("home-dashboard-plot", ix))
                .role(Role::Image)
                .aria_label(chart_summary(chart))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .children((top > 0.0).then(|| faint(scale_text(top, chart.unit.as_deref()))))
                .child(
                    div()
                        .relative()
                        .h(px(PANEL_CHART_HEIGHT))
                        .child(bars(theme, chart, PANEL_CHART_HEIGHT))
                        // The scale's line, at the top.
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(1.0))
                                .bg(theme.hairline(0.06)),
                        )
                        .children(threshold),
                )
                .child(baseline(theme))
                .child(x_labels),
        );
    frosted(panel)
}

/// The space between bars: less when there are many.
fn bar_gap(chart: &Chart) -> f32 {
    if chart.labels.len() > 16 { 2.0 } else { 4.0 }
}

/// Stacked bars `height` tall at the chart's scale, the first series at the
/// bottom. Each bar is the full height under the pointer, with its tooltip.
fn bars(theme: &Theme, chart: &Chart, height: f32) -> Div {
    let top = chart.scale();
    let totals = chart.totals();
    let wash = theme.wash(0.06);
    div()
        .h(px(height))
        .flex()
        .items_end()
        .gap(px(bar_gap(chart)))
        .children(totals.iter().enumerate().map(|(ix, total)| {
            let tall = if top > 0.0 {
                height * (total / top) as f32
            } else {
                0.0
            };
            let segments: Vec<(f64, Hsla)> = chart
                .series
                .iter()
                .rev()
                .filter(|series| series.values[ix] > 0.0)
                .map(|series| (series.values[ix], theme.chart_series(slot(&series.id))))
                .collect();
            let stack = div()
                .w_full()
                .h(px(tall))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .children(segments.into_iter().enumerate().map(|(n, (value, color))| {
                    div()
                        .w_full()
                        .flex_grow(value as f32)
                        .flex_basis(px(0.0))
                        .bg(color)
                        .when(n == 0, |el| el.rounded_t(px(2.0)))
                }));
            div()
                .id(("bar", ix))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .rounded_t(px(2.0))
                .hover(move |s| s.bg(wash))
                .tooltip(widgets::text_tooltip(bar_text(chart, ix)))
                .child(stack)
        }))
}

fn baseline(theme: &Theme) -> Div {
    div().h(px(1.0)).bg(theme.hairline(0.12))
}

/// One panel per table: its columns, then per row its label, its share
/// bar, its cells and its sub line.
fn breakdown_panel(theme: &Theme, table: &Breakdown, ix: usize) -> AnyElement {
    let columns = if table.columns.is_empty() {
        table
            .rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0)
    } else {
        table.columns.len()
    };
    let cell = || div().flex_none().w(px(CELL_WIDTH)).flex().justify_end();
    let header = div()
        .flex()
        .items_baseline()
        .gap(px(8.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(panel_title(theme, &table.title)),
        )
        .children(table.columns.iter().map(|column| {
            cell().child(
                div()
                    .truncate()
                    .text_size(ui_rems(10.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(column.clone())),
            )
        }));
    let rows = table.rows.iter().map(|row| {
        let share = row.share;
        div()
            .flex()
            .items_start()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(ui_rems(12.5))
                            .line_height(ui_rems(17.0))
                            .text_color(theme.text)
                            .child(SharedString::from(row.label.clone())),
                    )
                    .child(div().h(px(3.0)).rounded_full().bg(theme.wash(0.08)).when(
                        share > 0.0,
                        |el| {
                            el.child(
                                div()
                                    .h_full()
                                    .w(relative(share.max(0.015)))
                                    .rounded_full()
                                    .bg(theme.accent.opacity(0.75)),
                            )
                        },
                    ))
                    .children(row.sub.clone().map(|sub| {
                        div()
                            .truncate()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(sub))
                    })),
            )
            .children((0..columns).map(|ix| {
                cell().child(
                    div()
                        .text_size(ui_rems(11.5))
                        .line_height(ui_rems(17.0))
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.text)
                        .child(SharedString::from(
                            row.cells.get(ix).cloned().unwrap_or_default(),
                        )),
                )
            }))
    });
    let panel = card_surface(theme)
        .id(("home-dashboard-table", ix))
        .px(px(PANEL_PAD))
        .py(px(10.0))
        .gap(px(8.0))
        .child(header)
        .children(rows);
    frosted(panel)
}

fn panel_title(theme: &Theme, title: &str) -> Div {
    div()
        .truncate()
        .text_size(ui_rems(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(SharedString::from(title.to_string()))
}

/// A panel on the same frost as Home's cards.
fn frosted(panel: Stateful<Div>) -> AnyElement {
    crate::frost::frosted(super::view::CARD_RADIUS, crate::frost::MENU_BLUR, panel)
        .into_any_element()
}

/// Panels two across when `two`, the last alone at half width; else one
/// above the other. `None` when there are none.
fn panel_grid(panels: Vec<AnyElement>, two: bool) -> Option<Div> {
    if panels.is_empty() {
        return None;
    }
    let grid = div().flex().flex_col().gap(px(PANEL_GAP));
    if !two {
        return Some(grid.children(panels));
    }
    let mut rows = Vec::new();
    let mut panels = panels.into_iter();
    while let Some(first) = panels.next() {
        let second = panels.next();
        rows.push(
            div()
                .flex()
                .gap(px(PANEL_GAP))
                .child(div().flex_1().min_w_0().child(first))
                .child(div().flex_1().min_w_0().children(second)),
        );
    }
    Some(grid.children(rows))
}

/// What the mini couldn't count, as one quiet line.
fn problems(theme: &Theme, errors: &[String]) -> Option<Div> {
    (!errors.is_empty()).then(|| {
        div()
            .debug_selector(|| "home-dashboard-problems".to_string())
            .text_size(ui_rems(11.5))
            .line_height(ui_rems(16.0))
            .text_color(theme.text_faint)
            .line_clamp(2)
            .text_ellipsis()
            .child(SharedString::from(errors.join(" · ")))
    })
}
