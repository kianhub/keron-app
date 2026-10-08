//! The app's own data for `zeron:` widgets: a plain [`ZeronSnapshot`] taken
//! from [`AppState`], so keron-home needs neither gpui nor this crate.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use keron_home::zeron::{
    DeviceSnapshot, PrState, PullRequestItem, SessionItem, SessionStatus, ZeronSnapshot,
};
use zeron_proto::{AuthState, ChangeRequestState, ChatIndicator, HarnessId};

use crate::state::{AppState, chat_location};

/// The snapshot at `now`. Sessions are every non-idle chat the sidebar
/// lists; services and to-dos have no source in the app yet, so they stay
/// `None` (the widget says so instead of showing an empty list).
pub(crate) fn snapshot(state: &AppState, now: DateTime<Utc>) -> ZeronSnapshot {
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
