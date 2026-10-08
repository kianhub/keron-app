//! The Usage widget (`zeron:usage`): plan usage per agent account, from the
//! same cached accounts list Settings → Accounts and the composer's usage
//! ring use ([`AccountsSnapshotCache`]), and drawn with their meters.

use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Entity, SharedString, Task, div, prelude::*, px};
use keron_home::Meter;
use zeron_proto::AgentAccountsSnapshot;
use zeron_rpc::methods;

use crate::account_usage::{FORCE_MIN_INTERVAL, POLL_INTERVAL};
use crate::settings::accounts::{AccountsSnapshotCache, UsageLevel, usage_color, usage_level};
use crate::state::AppState;
use crate::theme::Theme;
use crate::typography::ui_rems;

/// This Mac's cached accounts list, when one has loaded.
pub(crate) fn cached_accounts(cx: &App) -> Option<&AgentAccountsSnapshot> {
    cx.try_global::<AccountsSnapshotCache>()?.0.get(&None)
}

/// Home's own load of the accounts list, for when the Usage widget shows
/// before anything else asked.
#[derive(Default)]
struct UsageLoad {
    asked: Option<Instant>,
    _task: Option<Task<()>>,
}

impl gpui::Global for UsageLoad {}

/// Whether this Mac's cached list should be asked for again: no account in
/// it has usage windows, or the newest were fetched more than `max_age` ago.
/// Pure.
pub(crate) fn is_stale(snapshot: &AgentAccountsSnapshot, now_ms: i64, max_age: Duration) -> bool {
    let newest = snapshot
        .accounts
        .iter()
        .filter(|account| !account.usage_windows.is_empty())
        .filter_map(|account| account.usage_fetched_at)
        .max();
    newest.is_none_or(|at| now_ms.saturating_sub(at) > max_age.as_millis() as i64)
}

/// The Usage widget shows: ask the engine for this Mac's accounts list the
/// way the composer's ring does, the plain list first (last known usage, no
/// network), then one forced usage probe. Nothing else keeps this Mac's list
/// fresh while Home shows (the ring polls only for an open chat's harness and
/// device), so this asks when none is cached, at most once per
/// [`FORCE_MIN_INTERVAL`], and when the cached one is stale ([`is_stale`]),
/// at most once per [`POLL_INTERVAL`], the ring's own pace. The engine
/// throttles forced probes too.
pub(crate) fn load_once(state: &Entity<AppState>, cx: &mut App) {
    let wait = match cached_accounts(cx) {
        None => FORCE_MIN_INTERVAL,
        Some(snapshot)
            if is_stale(
                snapshot,
                chrono::Utc::now().timestamp_millis(),
                POLL_INTERVAL,
            ) =>
        {
            POLL_INTERVAL
        }
        Some(_) => return,
    };
    if cx
        .try_global::<UsageLoad>()
        .and_then(|load| load.asked)
        .is_some_and(|at| at.elapsed() < wait)
    {
        return;
    }
    let Some(engine) = state.read(cx).engine().cloned() else {
        return;
    };
    let task = cx.spawn(async move |cx| {
        for force_usage in [false, true] {
            let reply = engine
                .client()
                .call(
                    methods::LIST_AGENT_ACCOUNTS,
                    serde_json::json!({ "forceUsage": force_usage }),
                )
                .await;
            if let Ok(Ok(snapshot)) = reply.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                cx.update(|cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(None, snapshot);
                });
            }
        }
    });
    cx.set_global(UsageLoad {
        asked: Some(Instant::now()),
        _task: Some(task),
    });
}

/// A usage row's meters: per window, its label, a bar that fills the card's
/// width, the percentage, and when it resets underneath. Same colors and
/// thresholds as Settings → Accounts. `None` for a row without any.
pub(super) fn meters(theme: &Theme, meters: &[Meter]) -> Option<AnyElement> {
    if meters.is_empty() {
        return None;
    }
    let now = chrono::Utc::now();
    Some(
        div()
            .mt(px(4.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(meters.iter().map(|meter| meter_row(theme, meter, now)))
            .into_any_element(),
    )
}

/// Room for "Session" / "Week" / "Month" at the meter's text size.
const METER_LABEL_WIDTH: f32 = 56.0;
const METER_PERCENT_WIDTH: f32 = 36.0;

fn meter_row(theme: &Theme, meter: &Meter, now: chrono::DateTime<chrono::Utc>) -> AnyElement {
    let fraction = meter.used.clamp(0.0, 1.0);
    let level = usage_level(fraction);
    let fill = usage_color(level, theme).opacity(match level {
        UsageLevel::Normal => 0.8,
        _ => 0.9,
    });
    let resets = meter
        .resets_at_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| resets_in(at, now));
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .text_size(ui_rems(11.5))
        .child(
            div()
                .h(px(16.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(METER_LABEL_WIDTH))
                        .flex_none()
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(SharedString::from(meter.label.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(4.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(theme.wash(0.08))
                        .when(fraction > 0.0, |el| {
                            // A 1.5% floor keeps tiny non-zero usage visible.
                            el.child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(fraction.max(0.015)))
                                    .rounded_full()
                                    .bg(fill),
                            )
                        }),
                )
                .child(
                    div()
                        .w(px(METER_PERCENT_WIDTH))
                        .flex_none()
                        .text_right()
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.text)
                        .child(SharedString::from(format!(
                            "{}%",
                            (fraction * 100.0).round()
                        ))),
                ),
        )
        .when_some(resets, |el, resets| {
            el.child(
                div()
                    .pl(px(METER_LABEL_WIDTH + 8.0))
                    .text_size(ui_rems(10.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(resets)),
            )
        })
        .into_any_element()
}

/// "resets in 42m", "resets in 2h 14m" within a day, else "resets Fri 09:00"
/// in local time.
fn resets_in(at: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> String {
    let left = at - now;
    let minutes = left.num_minutes();
    if minutes <= 0 {
        "resets now".to_string()
    } else if minutes < 60 {
        format!("resets in {minutes}m")
    } else if minutes < 24 * 60 {
        format!("resets in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!(
            "resets {}",
            at.with_timezone(&chrono::Local).format("%a %H:%M")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_times_read_short() {
        let now = chrono::DateTime::from_timestamp_millis(NOW).unwrap();
        let later = |minutes: i64| now + chrono::Duration::minutes(minutes);
        assert_eq!(resets_in(later(42), now), "resets in 42m");
        assert_eq!(resets_in(later(134), now), "resets in 2h 14m");
        assert_eq!(resets_in(later(-5), now), "resets now");
        assert!(resets_in(later(3 * 24 * 60), now).starts_with("resets "));
    }

    const NOW: i64 = 1_791_460_000_000;

    fn snapshot(fetched_ago_ms: &[(i64, bool)]) -> AgentAccountsSnapshot {
        let accounts: Vec<serde_json::Value> = fetched_ago_ms
            .iter()
            .enumerate()
            .map(|(i, (ago, windows))| {
                serde_json::json!({
                    "id": format!("claude-{i}"),
                    "harness": "claude-code",
                    "active": i == 0,
                    "switchable": true,
                    "usageFetchedAt": NOW - ago,
                    "usageWindows": if *windows {
                        serde_json::json!([{"label": "Session", "usedFraction": 0.5}])
                    } else {
                        serde_json::json!([])
                    },
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({ "accounts": accounts, "warnings": [] })).unwrap()
    }

    #[test]
    fn a_cached_list_is_stale_without_windows_or_once_old() {
        let max = Duration::from_secs(300);
        assert!(!is_stale(
            &snapshot(&[(400_000, true), (60_000, true)]),
            NOW,
            max
        ));
        assert!(is_stale(&snapshot(&[(400_000, true)]), NOW, max));
        // An offline launch: the forced probe failed, no windows at all.
        assert!(is_stale(&snapshot(&[(0, false)]), NOW, max));
        assert!(is_stale(&snapshot(&[]), NOW, max));
    }
}
