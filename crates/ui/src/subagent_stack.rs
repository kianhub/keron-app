//! The subagents a chat's run has at work, kept in view beside the composer
//! (Claude Code lists its background agents under its input the same way). A
//! spawn chip scrolls away with the transcript, and the right pane's
//! Subagents section is out of sight until opened; this stack stays put while
//! the run lasts.
//!
//! Where the window has room it is a quiet column in the empty gutter left of
//! the composer, its foot level with the pill's. Where it doesn't, it is a
//! footer chip ("2 agents") beside the usage rings, whose popover lists the
//! same entries. An entry opens its subagent's transcript in the right pane,
//! exactly as its spawn chip does; "+N more" and "N done" open the right
//! pane's Subagents section.
//!
//! Entries are read off the same spawn chips the transcript and the right
//! pane read, so the three always agree. Pure logic (which entries show, the
//! clock, where the stack goes) lives in free functions with unit tests.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use gpui::{
    AnyElement, App, Context, Entity, EntityId, EventEmitter, Hsla, IntoElement, PathBuilder,
    Pixels, Point, Render, SharedString, Subscription, Window, canvas, div, point, prelude::*, px,
};
use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry, SubagentStatus};

use crate::icons::{self, icon};
use crate::motion::{self, MotionSpec};
use crate::popover;
use crate::state::{AppState, Indicator};
use crate::theme::Theme;

/// Running entries shown before "+N more".
pub(crate) const MAX_ROWS: usize = 4;
/// Finished entries showing their check (or cross) at once.
pub(crate) const MAX_FINISHED_ROWS: usize = 2;
/// How long a finished subagent keeps its row before folding into "N done".
pub(crate) const LINGER_MS: i64 = 4_000;
/// Gap between the column's right edge and the composer pill.
pub(crate) const GUTTER_GAP: f32 = 12.0;
/// Room kept clear between the column and the edge of the chat column.
const GUTTER_EDGE: f32 = 20.0;
/// The column's width range. Below the minimum the stack moves to the
/// composer's footer as a chip.
const COLUMN_MIN: f32 = 150.0;
const COLUMN_MAX: f32 = 200.0;

/// The status ring and the slot it sits in.
const RING: f32 = 12.0;
const RING_STROKE: f32 = 1.5;
const RING_SLOT: f32 = 14.0;
/// Share of the ring the running arc covers.
const ARC_SWEEP: f32 = 0.28;
/// One turn of the running arc.
const SPIN: MotionSpec = MotionSpec::new(1000, motion::EASE);
/// A finished row folding into "N done".
const FOLD: MotionSpec = motion::COLLAPSE;

/// Row geometry, in px so a folding row knows the height it gives back.
const ROW_PAD_Y: f32 = 3.0;
const ROW_PAD_X: f32 = 8.0;
const ROW_GLYPH_GAP: f32 = 6.0;
const NAME_LINE: f32 = 16.0;
const ACTIVITY_LINE: f32 = 15.0;
/// The transparent border that turns accent under keyboard focus.
const ROW_BORDER: f32 = 1.0;
/// The chip's popover width.
const MENU_WIDTH: f32 = 260.0;

// ---------------------------------------------------------------------------
// Pure model
// ---------------------------------------------------------------------------

/// One spawn chip of the selected chat, as the transcript holds it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spawn {
    pub doc_id: String,
    pub title: SharedString,
    pub status: Option<SubagentStatus>,
    /// The spawn's live tail. Older docs fold one in; newer runs don't.
    pub tail: Option<String>,
    /// When the turn that spawned it was written, epoch ms.
    pub spawned_at: i64,
    /// Spawned (or steered) since the latest user message.
    pub current_run: bool,
}

/// The spawn chips of `transcript`, one per subagent doc, in spawn order.
/// Only genuine spawns with a stamped doc ref count, as in the right pane's
/// Subagents section; a steered subagent updates its entry in place.
pub(crate) fn spawns(transcript: &[SessionMessageEntry]) -> Vec<Spawn> {
    let run_start = transcript
        .iter()
        .rposition(|entry| entry.role == MessageRole::User)
        .map_or(0, |ix| ix + 1);
    let mut spawns: Vec<Spawn> = Vec::new();
    for (ix, entry) in transcript.iter().enumerate() {
        for part in &entry.parts {
            let MessagePart::Tool {
                call,
                subagent_ref: Some(doc_id),
                subagent_status,
                subagent_tail,
                ..
            } = part
            else {
                continue;
            };
            if !call.is_subagent_spawn() {
                continue;
            }
            let spawn = Spawn {
                doc_id: doc_id.clone(),
                title: crate::transcript::subagent_tab_title(call),
                status: *subagent_status,
                tail: subagent_tail.clone(),
                spawned_at: entry.created_at,
                current_run: ix >= run_start,
            };
            match spawns.iter_mut().find(|s| s.doc_id == spawn.doc_id) {
                Some(existing) => *existing = spawn,
                None => spawns.push(spawn),
            }
        }
    }
    spawns
}

/// What the stack remembers about one subagent between scans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Track {
    /// Clock base, epoch ms.
    started_at: i64,
    status: Option<SubagentStatus>,
    /// When this view saw it finish. `None` while it runs, and for one that
    /// had finished before the view was watching: that one never lingers.
    finished_at: Option<i64>,
}

fn is_finished(status: Option<SubagentStatus>) -> bool {
    matches!(
        status,
        Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
    )
}

/// Fold a fresh scan into `tracks`. `live` is false for a chat's first scan:
/// whatever is already there started with its turn and, if it has finished,
/// finished before anyone was watching. Later scans see changes as they
/// happen, at `now` (epoch ms).
pub(crate) fn track(tracks: &mut HashMap<String, Track>, spawns: &[Spawn], live: bool, now: i64) {
    tracks.retain(|doc_id, _| spawns.iter().any(|spawn| &spawn.doc_id == doc_id));
    for spawn in spawns {
        let Some(track) = tracks.get_mut(&spawn.doc_id) else {
            tracks.insert(
                spawn.doc_id.clone(),
                Track {
                    started_at: if live { now } else { spawn.spawned_at },
                    status: spawn.status,
                    finished_at: (live && is_finished(spawn.status)).then_some(now),
                },
            );
            continue;
        };
        if track.status == spawn.status {
            continue;
        }
        if is_finished(spawn.status) && !is_finished(track.status) {
            track.finished_at = Some(now);
        } else if spawn.status == Some(SubagentStatus::Running) && is_finished(track.status) {
            // Steered back to work: a fresh clock.
            track.started_at = now;
            track.finished_at = None;
        }
        track.status = spawn.status;
    }
}

/// One subagent as the stack shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Subagent {
    pub doc_id: String,
    pub title: SharedString,
    pub status: SubagentStatus,
    /// What it is doing now, when that is known.
    pub activity: Option<SharedString>,
    pub current_run: bool,
    /// Clock base, epoch ms.
    pub started_at: i64,
    /// When this view saw it finish, epoch ms (see [`Track::finished_at`]).
    pub finished_at: Option<i64>,
}

/// The tracked spawns with a known lifecycle, in spawn order. `activity`
/// supplies each one's live line.
pub(crate) fn subagents(
    spawns: &[Spawn],
    tracks: &HashMap<String, Track>,
    activity: impl Fn(&Spawn) -> Option<SharedString>,
) -> Vec<Subagent> {
    spawns
        .iter()
        .filter_map(|spawn| {
            let status = spawn.status?;
            let track = tracks.get(&spawn.doc_id)?;
            Some(Subagent {
                doc_id: spawn.doc_id.clone(),
                title: spawn.title.clone(),
                status,
                activity: activity(spawn),
                current_run: spawn.current_run,
                started_at: track.started_at,
                finished_at: track.finished_at,
            })
        })
        .collect()
}

/// What a subagent is doing now: its live tail, else how many tools its
/// transcript shows when the right pane already watches it.
pub(crate) fn activity(spawn: &Spawn, transcript: &[SessionMessageEntry]) -> Option<SharedString> {
    if let Some(tail) = spawn
        .tail
        .as_deref()
        .map(zeron_proto::view::single_line)
        .filter(|tail| !tail.is_empty())
    {
        return Some(tail.into());
    }
    let calls = transcript
        .iter()
        .flat_map(|entry| &entry.parts)
        .filter(|part| matches!(part, MessagePart::Tool { .. }))
        .count();
    match calls {
        0 => None,
        1 => Some("1 tool call".into()),
        n => Some(format!("{n} tool calls").into()),
    }
}

/// One entry row of the stack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub subagent: Subagent,
    /// 0 while shown; rises to 1 as a finished row folds into "N done".
    pub fold: f32,
}

/// What the stack shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Stack {
    /// Running subagents first, then the ones that just finished.
    pub rows: Vec<Row>,
    /// Running subagents past [`MAX_ROWS`].
    pub more: usize,
    /// This run's finished subagents that have folded away.
    pub done: usize,
    /// How many of `done` failed.
    pub failed: usize,
}

impl Stack {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.more == 0 && self.done == 0
    }

    /// Running subagents, shown or not.
    pub fn running(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.subagent.status == SubagentStatus::Running)
            .count()
            + self.more
    }

    /// Finished subagents, still on a row or folded into "N done".
    pub fn finished(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.subagent.status != SubagentStatus::Running && row.fold == 0.0)
            .count()
            + self.done
    }

    pub fn folding(&self) -> bool {
        self.rows.iter().any(|row| row.fold > 0.0)
    }
}

/// Which entries show at `now` (epoch ms). Running subagents take the rows
/// first, at most [`MAX_ROWS`], with the rest behind "+N more". A subagent
/// seen finishing keeps its row (at most [`MAX_FINISHED_ROWS`] at once,
/// first finished first) for [`LINGER_MS`], then folds over `fold_ms` into
/// "N done", which counts this run's finished subagents and lasts while the
/// run does or anything still runs. Rows keep spawn order, so a subagent
/// that finishes turns into its check in place.
pub(crate) fn stack(subagents: &[Subagent], run_active: bool, fold_ms: i64, now: i64) -> Stack {
    let running = subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .count();
    let mut shown: Vec<(&str, f32)> = subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .take(MAX_ROWS)
        .map(|s| (s.doc_id.as_str(), 0.0))
        .collect();
    // Finished subagents still lingering or folding, by how long ago they
    // finished. A row keeps its place until it has folded; one finishing past
    // the cap goes straight into "N done".
    let mut finishing: Vec<(&Subagent, i64)> = subagents
        .iter()
        .filter(|s| s.status != SubagentStatus::Running)
        .filter_map(|s| Some((s, now - s.finished_at?)))
        .filter(|(_, since)| *since < LINGER_MS + fold_ms)
        .collect();
    finishing.sort_by_key(|(_, since)| std::cmp::Reverse(*since));
    finishing.truncate(MAX_FINISHED_ROWS);
    shown.extend(finishing.iter().map(|(s, since)| {
        let fold = if *since >= LINGER_MS {
            (since - LINGER_MS) as f32 / fold_ms as f32
        } else {
            0.0
        };
        (s.doc_id.as_str(), fold)
    }));
    let rows = subagents
        .iter()
        .filter_map(|s| {
            let (_, fold) = shown.iter().find(|(id, _)| *id == s.doc_id)?;
            Some(Row {
                subagent: s.clone(),
                fold: *fold,
            })
        })
        .collect();
    let lingering = |s: &Subagent| {
        finishing
            .iter()
            .any(|(row, since)| row.doc_id == s.doc_id && *since < LINGER_MS)
    };
    let (mut done, mut failed) = (0, 0);
    if run_active || running > 0 {
        for s in subagents
            .iter()
            .filter(|s| s.current_run && s.status != SubagentStatus::Running && !lingering(s))
        {
            done += 1;
            if s.status == SubagentStatus::Failed {
                failed += 1;
            }
        }
    }
    Stack {
        rows,
        more: running.saturating_sub(MAX_ROWS),
        done,
        failed,
    }
}

/// A running subagent's clock: m:ss, or h:mm:ss past the hour.
pub(crate) fn clock(elapsed_ms: i64) -> String {
    let secs = (elapsed_ms / 1000).max(0);
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Where the stack goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Placement {
    /// A column this wide in the gutter left of the composer.
    Gutter(f32),
    /// A chip in the composer's footer row.
    Chip,
}

/// Where the stack goes, given the free width between the chat column's left
/// edge and the composer pill.
pub(crate) fn placement(gutter: f32) -> Placement {
    let room = gutter - GUTTER_GAP - GUTTER_EDGE;
    if room >= COLUMN_MIN {
        Placement::Gutter(room.min(COLUMN_MAX))
    } else {
        Placement::Chip
    }
}

fn agents_label(count: usize) -> String {
    if count == 1 {
        "1 agent".into()
    } else {
        format!("{count} agents")
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// What a click on the stack asks the shell for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SubagentStackEvent {
    /// Open this subagent's transcript in the right pane, as its spawn chip
    /// does.
    Open {
        doc_id: String,
        title: String,
        frozen: bool,
    },
    /// Open the right pane's Subagents section.
    OpenList,
}

/// The selected chat's subagent stack. The shell creates it and hands it to
/// the main composer, which mounts it in the gutter or the footer.
pub(crate) struct SubagentStack {
    state: Entity<AppState>,
    chat_id: Option<String>,
    /// This chat's first replayed transcript has been scanned.
    live: bool,
    revision: Option<u64>,
    tracks: HashMap<String, Track>,
    subagents: Vec<Subagent>,
    run_active: bool,
    placement: Placement,
    popup: popover::Popup<()>,
    /// The 1s clock tick is scheduled.
    ticking: bool,
    /// What the last sync showed, so a state change that leaves the stack as
    /// it was doesn't repaint it.
    last: Stack,
    _state: Subscription,
}

impl EventEmitter<SubagentStackEvent> for SubagentStack {}

impl SubagentStack {
    pub(crate) fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut stack = Self {
            _state: cx.observe(&state, |stack: &mut Self, _, cx| stack.sync(cx)),
            state,
            chat_id: None,
            live: false,
            revision: None,
            tracks: HashMap::new(),
            subagents: Vec::new(),
            run_active: false,
            placement: Placement::Chip,
            popup: popover::Popup::default(),
            ticking: false,
            last: Stack::default(),
        };
        stack.sync(cx);
        stack
    }

    pub(crate) fn placement(&self) -> Placement {
        self.placement
    }

    /// The free width left of the composer pill. Returns whether the stack
    /// changed between the gutter and the footer (the composer then remounts
    /// it).
    pub(crate) fn set_gutter(&mut self, gutter: f32, cx: &mut Context<Self>) -> bool {
        let next = placement(gutter);
        if next == self.placement {
            return false;
        }
        let moved = matches!(
            (self.placement, next),
            (Placement::Chip, Placement::Gutter(_)) | (Placement::Gutter(_), Placement::Chip)
        );
        if moved {
            self.popup = popover::Popup::default();
        }
        self.placement = next;
        cx.notify();
        moved
    }

    fn current(&self, now: i64, cx: &App) -> Stack {
        let fold_ms = if motion::reduced_motion(cx) {
            0
        } else {
            FOLD.total().as_millis() as i64
        };
        stack(&self.subagents, self.run_active, fold_ms, now)
    }

    /// Re-read the selected chat: rescan its spawn chips when the transcript
    /// moved, and repaint when what the stack shows changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let now = Utc::now();
        let state = self.state.read(cx);
        if state.selected_chat != self.chat_id {
            self.chat_id = state.selected_chat.clone();
            self.live = false;
            self.revision = None;
            self.tracks.clear();
            self.subagents.clear();
            self.popup = popover::Popup::default();
        }
        self.run_active = self.chat_id.as_deref().is_some_and(|chat_id| {
            matches!(
                state.indicator_for(chat_id, now),
                Indicator::Working | Indicator::AwaitingInput
            )
        });
        // Before the replay lands the transcript is empty or only echoes;
        // scanning it would read every spawn as newly seen.
        if self.chat_id.is_some()
            && state.transcript_replayed
            && self.revision != Some(state.transcript_revision)
        {
            self.revision = Some(state.transcript_revision);
            let spawns = spawns(&state.transcript);
            track(&mut self.tracks, &spawns, self.live, now.timestamp_millis());
            self.live = true;
            self.subagents = subagents(&spawns, &self.tracks, |spawn| {
                activity(spawn, state.sub_transcript(&spawn.doc_id))
            });
        }
        let shown = self.current(now.timestamp_millis(), cx);
        if shown != self.last {
            self.last = shown;
            cx.notify();
        }
    }

    /// Tick once a second while anything shows: the clocks advance, finished
    /// rows fold away, and a run that went quiet is noticed.
    fn ensure_tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let showing = this
                    .update(cx, |stack, cx| {
                        stack.sync(cx);
                        let showing = !stack.last.is_empty();
                        if showing {
                            cx.notify();
                        } else {
                            stack.ticking = false;
                        }
                        showing
                    })
                    .unwrap_or(false);
                if !showing {
                    break;
                }
            }
        })
        .detach();
    }

    fn open(&mut self, event: SubagentStackEvent, cx: &mut Context<Self>) {
        self.dismiss(cx);
        cx.emit(event);
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.is_open() {
            self.dismiss(cx);
        } else {
            self.popup.open(());
            cx.notify();
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |stack: &mut Self| &mut stack.popup);
        }
        cx.notify();
    }

    /// The entries, "+N more" and "N done", top to bottom. `scope` keys the
    /// ids, so the column and the popover never share element state.
    fn entries(
        &self,
        stack: &Stack,
        scope: &'static str,
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let view = cx.entity_id();
        let mut entries: Vec<AnyElement> = stack
            .rows
            .iter()
            .map(|row| self.entry(row, scope, now, view, theme, cx))
            .collect();
        if stack.more > 0 {
            entries.push(
                quiet_row(format!("{scope}-more"), theme)
                    .aria_label(SharedString::from(format!(
                        "{} more subagents running, open the list",
                        stack.more
                    )))
                    .on_click(cx.listener(|stack, _, _, cx| {
                        cx.stop_propagation();
                        stack.open(SubagentStackEvent::OpenList, cx);
                    }))
                    .child(name_line(
                        div().size(px(RING_SLOT)).flex_none().into_any_element(),
                        div()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(format!("+{} more", stack.more))),
                        None,
                        theme,
                    ))
                    .into_any_element(),
            );
        }
        if stack.done > 0 {
            let failed = (stack.failed > 0).then(|| {
                div()
                    .flex_none()
                    .text_color(theme.danger)
                    .child(SharedString::from(format!("· {} failed", stack.failed)))
            });
            entries.push(
                quiet_row(format!("{scope}-done"), theme)
                    .aria_label(SharedString::from(format!(
                        "{} subagents done, open the list",
                        stack.done
                    )))
                    .on_click(cx.listener(|stack, _, _, cx| {
                        cx.stop_propagation();
                        stack.open(SubagentStackEvent::OpenList, cx);
                    }))
                    .child(name_line(
                        status_glyph(SubagentStatus::Done, theme, view, cx),
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(format!("{} done", stack.done)))
                            .children(failed),
                        None,
                        theme,
                    ))
                    .into_any_element(),
            );
        }
        entries
    }

    fn entry(
        &self,
        row: &Row,
        scope: &'static str,
        now: i64,
        view: EntityId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let subagent = &row.subagent;
        let id = format!("{scope}-{}", subagent.doc_id);
        let running = subagent.status == SubagentStatus::Running;
        let clock = running.then(|| clock(now - subagent.started_at));
        let state = match (&clock, subagent.status) {
            (Some(clock), _) => format!("running for {clock}"),
            (None, SubagentStatus::Failed) => "failed".into(),
            (None, _) => "done".into(),
        };
        let event = SubagentStackEvent::Open {
            doc_id: subagent.doc_id.clone(),
            title: subagent.title.to_string(),
            frozen: !running,
        };
        let has_activity = subagent.activity.is_some();
        let body = quiet_row(id.clone(), theme)
            .aria_label(SharedString::from(format!(
                "Open subagent {}, {state}",
                subagent.title
            )))
            .on_click(cx.listener(move |stack, _, _, cx| {
                cx.stop_propagation();
                stack.open(event.clone(), cx);
            }))
            .child(name_line(
                status_glyph(subagent.status, theme, view, cx),
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(subagent.title.clone()),
                clock,
                theme,
            ))
            .when_some(subagent.activity.clone(), |row, activity| {
                row.child(
                    div()
                        .h(px(ACTIVITY_LINE))
                        .pl(px(RING_SLOT + ROW_GLYPH_GAP))
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.0))
                        .line_height(px(ACTIVITY_LINE))
                        .text_color(theme.text_faint)
                        .child(activity),
                )
            });
        if row.fold > 0.0 {
            // Fold away: give the height back as the row fades, so the rows
            // above settle onto the "N done" line instead of jumping.
            let t = FOLD.progress(row.fold);
            let height = row_height(has_activity);
            return div()
                .flex_none()
                .h(px(height * (1.0 - t)))
                .overflow_hidden()
                .opacity(1.0 - t)
                .child(body)
                .into_any_element();
        }
        motion::fade_in(SharedString::from(format!("{id}-in")), body).into_any_element()
    }

    /// The column in the gutter left of the composer.
    fn render_column(
        &self,
        stack: &Stack,
        width: f32,
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("subagent-stack")
            .w(px(width))
            .flex()
            .flex_col()
            .children(self.entries(stack, "subagent-stack", now, theme, cx))
            .into_any_element()
    }

    /// The footer chip, and its popover with the same entries.
    fn render_chip(
        &self,
        stack: &Stack,
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = stack.running();
        let view = cx.entity_id();
        let (glyph, label) = if running > 0 {
            (
                status_glyph(SubagentStatus::Running, theme, view, cx),
                agents_label(running),
            )
        } else {
            (
                status_glyph(SubagentStatus::Done, theme, view, cx),
                format!("{} done", stack.finished()),
            )
        };
        let open = self.popup.get().is_some();
        let chip = div()
            .id("subagent-stack-chip")
            .relative()
            .ml(px(4.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.0))
            .h(px(24.0))
            .px(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("Subagents: {label}")))
            .aria_expanded(open)
            .focus_visible(|s| s.border_color(theme.accent))
            .when(open, |s| s.bg(crate::theme::ink(0.05)))
            .hover(|s| s.bg(crate::theme::ink(0.05)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|stack, _, _, _| stack.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|stack, _, _, cx| stack.toggle(cx)))
            .child(glyph)
            .child(SharedString::from(label));
        if !open {
            return chip.into_any_element();
        }
        let popup = theme.for_popup();
        let content = popover::popover_card(&popup)
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .child(popover::menu_heading(&popup, "Subagents"))
            .children(self.entries(stack, "subagent-stack-menu", now, &popup, cx))
            .on_mouse_down_out(cx.listener(|stack, _, _, cx| stack.dismiss(cx)))
            .into_any_element();
        chip.child(popover::anchored_menu_above_end(
            "subagent-stack-menu",
            content,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }
}

impl Render for SubagentStack {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Utc::now().timestamp_millis();
        let stack = self.current(now, cx);
        self.last = stack.clone();
        if stack.is_empty() {
            self.popup = popover::Popup::default();
            return div().into_any_element();
        }
        self.ensure_tick(cx);
        if stack.folding() {
            window.request_animation_frame();
        }
        let theme = Theme::of(cx).clone();
        match self.placement {
            Placement::Gutter(width) => self.render_column(&stack, width, now, &theme, cx),
            Placement::Chip => self.render_chip(&stack, now, &theme, cx),
        }
    }
}

/// A full row's height, the figure a folding row shrinks from.
fn row_height(has_activity: bool) -> f32 {
    2.0 * (ROW_PAD_Y + ROW_BORDER) + NAME_LINE + if has_activity { ACTIVITY_LINE } else { 0.0 }
}

/// A clickable stack row: quiet text on the window, a light wash on hover,
/// an accent outline under keyboard focus.
fn quiet_row(id: String, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .role(gpui::Role::Button)
        .tab_index(0)
        .flex_none()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .px(px(ROW_PAD_X))
        .py(px(ROW_PAD_Y))
        .rounded(px(6.0))
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .text_color(theme.text_muted.opacity(0.8))
        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
        .focus_visible(|s| s.border_color(theme.accent))
}

/// A row's first line: status glyph, the truncating name, and the clock.
fn name_line(
    glyph: AnyElement,
    name: gpui::Div,
    clock: Option<String>,
    theme: &Theme,
) -> gpui::Div {
    div()
        .h(px(NAME_LINE))
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GLYPH_GAP))
        .text_size(crate::typography::ui_rems(12.0))
        .line_height(px(NAME_LINE))
        .child(glyph)
        .child(name.flex_1().min_w_0().truncate())
        .when_some(clock, |line, clock| {
            line.child(
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(clock)),
            )
        })
}

/// The status slot: a spinning accent arc while running, a check when done,
/// a cross when failed.
fn status_glyph(status: SubagentStatus, theme: &Theme, view: EntityId, cx: &mut App) -> AnyElement {
    let glyph = match status {
        SubagentStatus::Running => {
            // Static under reduced motion: the phase stays at 0.
            spin_ring(motion::pulse_delta(&SPIN, view, cx), theme).into_any_element()
        }
        SubagentStatus::Done => icon(icons::CHECK)
            .size(px(11.0))
            .text_color(theme.success)
            .into_any_element(),
        SubagentStatus::Failed => icon(icons::CLOSE)
            .size(px(10.0))
            .text_color(theme.danger)
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(RING_SLOT))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

/// A faint track under an accent arc turned to `phase` of a full turn.
fn spin_ring(phase: f32, theme: &Theme) -> impl IntoElement {
    let track = theme.text_faint.opacity(0.25);
    let arc = theme.accent;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = (RING - RING_STROKE) / 2.0;
            stroke_arc(window, center, radius, 0.0, 1.0, track);
            stroke_arc(window, center, radius, phase, ARC_SWEEP, arc);
        },
    )
    .size(px(RING))
}

/// Stroke `sweep` of a full turn, clockwise from `start` turns past twelve
/// o'clock. gpui paths have no arc primitive, so it is a polyline.
fn stroke_arc(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    start: f32,
    sweep: f32,
    color: Hsla,
) {
    let steps = ((48.0 * sweep).ceil() as usize).max(2);
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    for i in 0..=steps {
        let turn = start + sweep * i as f32 / steps as f32;
        let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * turn;
        let p = point(
            center.x + px(radius * angle.cos()),
            center.y + px(radius * angle.sin()),
        );
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageStatus;
    use zeron_proto::ToolCall;

    const FOLD_MS: i64 = 180;

    fn sub(id: &str, status: SubagentStatus, finished_at: Option<i64>) -> Subagent {
        Subagent {
            doc_id: id.into(),
            title: id.to_owned().into(),
            status,
            activity: None,
            current_run: true,
            started_at: 0,
            finished_at,
        }
    }

    fn ids(stack: &Stack) -> Vec<&str> {
        stack
            .rows
            .iter()
            .map(|row| row.subagent.doc_id.as_str())
            .collect()
    }

    fn spawn_part(doc: &str, status: SubagentStatus) -> MessagePart {
        MessagePart::Tool {
            id: format!("tool-{doc}"),
            call: ToolCall::Unknown {
                name: format!("Agent: {doc}"),
                input: None,
            },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: Some(doc.into()),
            subagent_status: Some(status),
            subagent_tail: None,
        }
    }

    fn turn(id: &str, role: MessageRole, at: i64, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            duration_ms: None,
            id: id.into(),
            role,
            parts,
            created_at: at,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        }
    }

    #[test]
    fn running_subagents_take_four_rows_and_the_rest_wait_behind_more() {
        let mut all: Vec<Subagent> = (1..=6)
            .map(|n| sub(&format!("run{n}"), SubagentStatus::Running, None))
            .collect();
        // Three just finished: the first two keep their marks, in spawn
        // order, the third goes straight into "N done".
        all.insert(1, sub("first", SubagentStatus::Failed, Some(8_000)));
        all.insert(3, sub("second", SubagentStatus::Done, Some(9_000)));
        all.push(sub("third", SubagentStatus::Done, Some(9_500)));
        let shown = stack(&all, true, FOLD_MS, 10_000);
        assert_eq!(
            ids(&shown),
            ["run1", "first", "run2", "second", "run3", "run4"]
        );
        assert_eq!((shown.more, shown.done, shown.running()), (2, 1, 6));
        assert_eq!(shown.finished(), 3);
    }

    #[test]
    fn a_finished_subagent_lingers_then_folds_into_done() {
        let all = [
            sub("a", SubagentStatus::Running, None),
            sub("b", SubagentStatus::Failed, Some(1_000)),
        ];
        let at = |now| stack(&all, true, FOLD_MS, now);
        let lingering = at(1_000 + LINGER_MS - 1);
        assert_eq!(ids(&lingering), ["a", "b"]);
        assert_eq!(lingering.rows[1].fold, 0.0);
        assert_eq!(lingering.done, 0);

        // Folding: still drawn while it shrinks, already counted as done.
        let folding = at(1_000 + LINGER_MS + FOLD_MS / 2);
        assert_eq!(ids(&folding), ["a", "b"]);
        assert!((folding.rows[1].fold - 0.5).abs() < 0.01);
        assert_eq!((folding.done, folding.failed), (1, 1));

        let folded = at(1_000 + LINGER_MS + FOLD_MS);
        assert_eq!(ids(&folded), ["a"]);
        assert_eq!((folded.done, folded.failed), (1, 1));

        // Reduced motion: no fold, the row simply goes.
        let still = stack(&all, true, 0, 1_000 + LINGER_MS);
        assert_eq!(ids(&still), ["a"]);
    }

    #[test]
    fn done_counts_this_run_and_clears_when_the_run_ends() {
        // A done subagent from an earlier turn, then this run's two.
        let transcript = vec![
            turn("u1", MessageRole::User, 0, vec![]),
            turn(
                "a1",
                MessageRole::Assistant,
                1_000,
                vec![spawn_part("old", SubagentStatus::Done)],
            ),
            turn("u2", MessageRole::User, 60_000, vec![]),
            turn(
                "a2",
                MessageRole::Assistant,
                61_000,
                vec![
                    spawn_part("this", SubagentStatus::Done),
                    spawn_part("still", SubagentStatus::Running),
                ],
            ),
        ];
        let found = spawns(&transcript);
        let mut tracks = HashMap::new();
        track(&mut tracks, &found, false, 70_000);
        let all = subagents(&found, &tracks, |_| None);
        // Running clocks start with the turn that spawned them.
        assert_eq!(all[2].started_at, 61_000);

        let shown = stack(&all, true, FOLD_MS, 70_000);
        assert_eq!(ids(&shown), ["still"]);
        assert_eq!(shown.done, 1);
        // Something still running keeps "N done" after the turn ends...
        assert_eq!(stack(&all, false, FOLD_MS, 70_000).done, 1);
        // ...and once nothing runs and the run is over, nothing shows.
        let finished: Vec<Subagent> = all
            .into_iter()
            .filter(|s| s.status != SubagentStatus::Running)
            .collect();
        assert!(stack(&finished, false, FOLD_MS, 70_000).is_empty());
    }

    #[test]
    fn only_a_finish_seen_live_lingers() {
        let transcript = |status| {
            vec![turn(
                "a",
                MessageRole::Assistant,
                1_000,
                vec![spawn_part("x", status)],
            )]
        };
        // Opening a chat whose subagent already finished: no check lingers.
        let found = spawns(&transcript(SubagentStatus::Done));
        let mut tracks = HashMap::new();
        track(&mut tracks, &found, false, 50_000);
        let shown = stack(&subagents(&found, &tracks, |_| None), true, FOLD_MS, 50_000);
        assert!(shown.rows.is_empty());
        assert_eq!(shown.done, 1);

        // Watching it run, then finish: the check stays for the linger.
        let mut tracks = HashMap::new();
        let running = spawns(&transcript(SubagentStatus::Running));
        track(&mut tracks, &running, false, 50_000);
        let finished = spawns(&transcript(SubagentStatus::Done));
        track(&mut tracks, &finished, true, 52_000);
        let all = subagents(&finished, &tracks, |_| None);
        assert_eq!(ids(&stack(&all, true, FOLD_MS, 53_000)), ["x"]);
        assert!(
            stack(&all, true, FOLD_MS, 52_000 + LINGER_MS + FOLD_MS)
                .rows
                .is_empty()
        );
    }

    #[test]
    fn clock_reads_minutes_and_seconds() {
        assert_eq!(clock(-5_000), "0:00");
        assert_eq!(clock(9_999), "0:09");
        assert_eq!(clock(75_000), "1:15");
        assert_eq!(clock(59 * 60_000 + 59_000), "59:59");
        assert_eq!(clock(3_725_000), "1:02:05");
    }

    #[test]
    fn a_narrow_gutter_moves_the_stack_into_the_footer() {
        // Plenty of room: the column caps at its widest.
        assert_eq!(placement(400.0), Placement::Gutter(COLUMN_MAX));
        // Just enough: it takes what there is, short of the gap and edge.
        let tight = COLUMN_MIN + GUTTER_GAP + GUTTER_EDGE;
        assert_eq!(placement(tight), Placement::Gutter(COLUMN_MIN));
        assert_eq!(placement(tight - 1.0), Placement::Chip);
        // A right pane squeezing the chat leaves only the pill's own inset.
        assert_eq!(placement(16.0), Placement::Chip);
    }
}
