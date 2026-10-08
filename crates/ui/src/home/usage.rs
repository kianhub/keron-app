//! The Usage widget (`zeron:usage`): plan usage per agent account, from the
//! same cached accounts list Settings → Accounts and the composer's usage
//! ring use ([`AccountsSnapshotCache`]), and drawn with their meters.

use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Entity, Task, div, prelude::*, px};
use keron_home::Meter;
use zeron_proto::{AgentAccountsSnapshot, AgentUsageWindow};
use zeron_rpc::methods;

use crate::account_usage::{FORCE_MIN_INTERVAL, POLL_INTERVAL};
use crate::settings::accounts::{AccountsSnapshotCache, render_usage_meter};
use crate::state::AppState;
use crate::theme::Theme;

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

/// A usage row's meters, one line per window, drawn like Settings →
/// Accounts (same colors and thresholds); `None` for a row without any.
pub(super) fn meters(theme: &Theme, meters: &[Meter]) -> Option<AnyElement> {
    if meters.is_empty() {
        return None;
    }
    Some(
        div()
            .mt(px(3.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(meters.iter().map(|meter| {
                render_usage_meter(
                    &AgentUsageWindow {
                        label: meter.label.clone(),
                        used_fraction: meter.used,
                        resets_at: meter
                            .resets_at_ms
                            .and_then(chrono::DateTime::from_timestamp_millis),
                    },
                    theme,
                )
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
