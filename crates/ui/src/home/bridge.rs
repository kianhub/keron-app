//! The app's own data for `zeron:` widgets: a plain [`ZeronSnapshot`] taken
//! from [`AppState`], so keron-home needs neither gpui nor this crate.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use keron_home::zeron::{
    self, DeviceSnapshot, PrState, PullRequestItem, SessionItem, SessionStatus, UsageAccount,
    UsageWindow, ZeronSnapshot,
};
use zeron_proto::{AgentAccountsSnapshot, AuthState, ChangeRequestState, ChatIndicator, HarnessId};

use crate::state::{AppState, chat_location};

/// The snapshot at `now`. Sessions are every non-idle chat the sidebar
/// lists; services and to-dos have no source in the app yet, so they stay
/// `None` (the widget says so instead of showing an empty list). `accounts`
/// is this Mac's cached agent accounts list (Settings → Accounts and the
/// composer's usage ring fill it), `None` until one has loaded.
pub(crate) fn snapshot(
    state: &AppState,
    accounts: Option<&AgentAccountsSnapshot>,
    now: DateTime<Utc>,
) -> ZeronSnapshot {
    let overview = state.overview_chats(now);
    let mut working: HashMap<&str, u32> = HashMap::new();
    for (status, chat) in &overview {
        if *status == ChatIndicator::Working {
            *working.entry(chat.device_id.as_str()).or_default() += 1;
        }
    }
    let sessions = overview
        .iter()
        .filter(|(status, _)| *status != ChatIndicator::Idle)
        .map(|(status, chat)| {
            let session = state.session_for(&chat.id);
            // The status strip's rule: a send in flight counts from when it
            // was sent.
            let started = session
                .and_then(|session| session.started_at)
                .into_iter()
                .chain(state.pending_send_started(&chat.id, now))
                .max();
            let last_activity = chat
                .last_message_at
                .unwrap_or(chat.created_at)
                .max(started.unwrap_or(chat.created_at));
            SessionItem {
                chat_id: chat.id.clone(),
                title: chat
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| "New session".to_string()),
                location: chat_location(chat),
                harness: chat
                    .config
                    .as_ref()
                    .map(|config| harness_name(config.harness).to_string()),
                device_name: state.device_name(&chat.device_id).map(str::to_string),
                status: session_status(*status),
                started_at_ms: started.map(|at| at.timestamp_millis()),
                last_activity_ms: last_activity.timestamp_millis(),
            }
        })
        .collect();
    let devices = state
        .devices
        .iter()
        .map(|device| DeviceSnapshot {
            id: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            online: state.device_online(&device.id, now),
            last_seen_ms: device.last_seen_at.map(|at| at.timestamp_millis()),
            is_self: state.local_device_id.as_deref() == Some(device.id.as_str()),
            working_sessions: working.get(device.id.as_str()).copied().unwrap_or(0),
        })
        .collect();
    // Several chats can sit on one branch, so one pull request shows once.
    let mut seen = HashSet::new();
    let pull_requests = state
        .chats
        .iter()
        .filter(|chat| !chat.archived)
        .filter_map(|chat| {
            let summary = state.change_request_for_chat(chat)?;
            seen.insert(summary.url.clone()).then(|| PullRequestItem {
                chat_id: chat.id.clone(),
                number: summary.number,
                title: summary.title.clone(),
                url: summary.url.clone(),
                repo: repo_from_url(&summary.url),
                state: match summary.state {
                    ChangeRequestState::Open => PrState::Open,
                    ChangeRequestState::Merged => PrState::Merged,
                    ChangeRequestState::Closed => PrState::Closed,
                },
                head_ref: summary.head_ref.clone(),
            })
        })
        .collect();
    ZeronSnapshot {
        taken_at_ms: now.timestamp_millis(),
        signed_in: matches!(state.auth, Some(AuthState::SignedIn { .. })),
        sessions,
        devices,
        pull_requests,
        services: None,
        todos: None,
        usage: accounts.map(usage_accounts),
    }
}

/// The accounts that report plan usage, as plain data: the provider's name
/// as the owner knows it, the plan, and the email cut down to its first part.
pub(crate) fn usage_accounts(snapshot: &AgentAccountsSnapshot) -> Vec<UsageAccount> {
    snapshot
        .accounts
        .iter()
        .filter(|account| crate::settings::accounts::reports_usage(account.harness))
        .map(|account| UsageAccount {
            harness: usage_name(account.harness).to_string(),
            plan: account
                .plan_label
                .clone()
                .filter(|plan| !plan.trim().is_empty()),
            who: zeron::who(account.email.as_deref()),
            active: account.active,
            windows: account
                .usage_windows
                .iter()
                .map(|window| UsageWindow {
                    label: window.label.clone(),
                    used: window.used_fraction,
                    resets_at_ms: window.resets_at.map(|at| at.timestamp_millis()),
                })
                .collect(),
            fetched_at_ms: account.usage_fetched_at,
            error: account.usage_error.clone(),
        })
        .collect()
}

/// A plan's provider as the owner calls it: Claude Code runs on a Claude
/// plan and Codex on a ChatGPT one.
fn usage_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude",
        HarnessId::Codex => "ChatGPT",
        other => harness_name(other),
    }
}

fn session_status(status: ChatIndicator) -> SessionStatus {
    match status {
        ChatIndicator::Working => SessionStatus::Working,
        ChatIndicator::AwaitingInput => SessionStatus::AwaitingInput,
        ChatIndicator::Errored => SessionStatus::Errored,
        ChatIndicator::Completed => SessionStatus::Completed,
        ChatIndicator::Idle => SessionStatus::Idle,
    }
}

fn harness_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Devin => "Devin",
        HarnessId::Grok => "Grok",
        HarnessId::Hermes => "Hermes",
        HarnessId::Pi => "Pi",
        HarnessId::Opencode => "OpenCode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Mock => "Mock",
    }
}

/// "owner/repo" from a pull request's web address.
fn repo_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let mut segments = parsed.path_segments()?;
    let owner = segments.next().filter(|part| !part.is_empty())?;
    let repo = segments.next().filter(|part| !part.is_empty())?;
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use keron_home::Body;
    use zeron_proto::AgentAccountsSnapshot;

    use super::*;

    fn accounts() -> AgentAccountsSnapshot {
        let account = |harness: &str, email: &str, plan: Option<&str>, active: bool| {
            serde_json::json!({
                "id": format!("{harness}-{email}"),
                "harness": harness,
                "email": email,
                "planLabel": plan,
                "active": active,
                "switchable": true,
                "usageFetchedAt": 1_791_460_000_000_i64,
                "usageWindows": [
                    {"label": "Session", "usedFraction": 0.42, "resetsAt": "2026-10-08T12:00:00Z"},
                    {"label": "Week", "usedFraction": 0.97, "resetsAt": null},
                ],
            })
        };
        serde_json::from_value(serde_json::json!({
            "accounts": [
                account("claude-code", "kian@example.com", Some("Max"), false),
                account("codex", "kian@example.com", Some("Pro"), true),
                account("antigravity", "kian@example.com", None, true),
            ],
            "warnings": [],
        }))
        .unwrap()
    }

    #[test]
    fn usage_comes_from_the_accounts_list_without_emails() {
        let usage = usage_accounts(&accounts());
        let summary: Vec<(&str, Option<&str>, &str, bool)> = usage
            .iter()
            .map(|a| {
                (
                    a.harness.as_str(),
                    a.plan.as_deref(),
                    a.who.as_str(),
                    a.active,
                )
            })
            .collect();
        // Antigravity reports no usage, so it isn't listed.
        assert_eq!(
            summary,
            [
                ("Claude", Some("Max"), "kian", false),
                ("ChatGPT", Some("Pro"), "kian", true),
            ]
        );
        let windows = &usage[0].windows;
        assert_eq!(windows[0].label, "Session");
        assert!((windows[0].used - 0.42).abs() < 1e-6);
        assert_eq!(
            windows[0].resets_at_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-08T12:00:00Z")
                    .unwrap()
                    .timestamp_millis()
            )
        );
        assert_eq!(windows[1].resets_at_ms, None);
        assert_eq!(usage[0].fetched_at_ms, Some(1_791_460_000_000));

        let snapshot = ZeronSnapshot {
            usage: Some(usage),
            ..ZeronSnapshot::default()
        };
        let Body::List(rows) = keron_home::zeron::payload("usage", &snapshot, 1_791_460_060_000)
            .unwrap()
            .body
        else {
            panic!("not a list")
        };
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["ChatGPT · Pro", "Claude · Max"]);
        assert!(
            rows.iter()
                .all(|row| !format!("{row:?}").contains("example.com"))
        );
    }
}
