//! `zeron:<name>` sources: data the app already has, as a plain snapshot
//! the ui crate fills from its AppState (crates/ui/src/home/bridge.rs), so
//! this crate needs neither gpui nor the ui crate.
//!
//! Names: `sessions` (working and waiting chats), `devices`,
//! `pull-requests`, `services`, `todos`. `services` and `todos` have no
//! source in the app yet; their snapshot fields are `None` and the payload
//! says so instead of showing an empty list.

use std::collections::HashSet;

use serde::Serialize;

use crate::kinds::{chat_link, short_age_secs};
use crate::{Body, DeviceItem, ListItem, Payload};

/// The names a `zeron:` source may use.
pub const NAMES: &[&str] = &["sessions", "devices", "pull-requests", "services", "todos"];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ZeronSnapshot {
    /// When it was taken, ms since the epoch.
    pub taken_at_ms: i64,
    /// Whether the app is signed in to the relay (synced workspace).
    pub signed_in: bool,
    pub sessions: Vec<SessionItem>,
    pub devices: Vec<DeviceSnapshot>,
    pub pull_requests: Vec<PullRequestItem>,
    pub services: Option<Vec<ServiceItem>>,
    pub todos: Option<Vec<TodoList>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStatus {
    Working,
    AwaitingInput,
    Errored,
    Completed,
    #[default]
    Idle,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SessionItem {
    pub chat_id: String,
    pub title: String,
    /// "project · branch".
    pub location: Option<String>,
    /// "Claude Code", "Codex"...
    pub harness: Option<String>,
    pub device_name: Option<String>,
    pub status: SessionStatus,
    pub started_at_ms: Option<i64>,
    pub last_activity_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DeviceSnapshot {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
    pub last_seen_ms: Option<i64>,
    pub is_self: bool,
    pub working_sessions: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    #[default]
    Open,
    Merged,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PullRequestItem {
    pub chat_id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// "owner/repo", from the URL.
    pub repo: Option<String>,
    pub state: PrState,
    pub head_ref: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ServiceItem {
    pub name: String,
    pub project: String,
    pub port: u16,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TodoList {
    pub chat_id: String,
    pub title: String,
    pub items: Vec<(String, bool)>,
}

/// The payload for `zeron:<name>` at `now_ms`, or a short reason when that
/// source has nothing to give (an unknown name, or no source in the app yet).
/// `sessions` lists working, waiting and errored chats, newest activity
/// first, with an elapsed age, a `status` and a [`crate::kinds::chat_link`].
/// `devices` lists every device with this Mac first. `pull-requests` lists
/// open ones first, deduplicated by URL.
pub fn payload(name: &str, snapshot: &ZeronSnapshot, now_ms: i64) -> Result<Payload, String> {
    let body = match name {
        "sessions" => Body::List(sessions(&snapshot.sessions, now_ms)),
        "devices" => Body::Devices(devices(&snapshot.devices, now_ms)),
        "pull-requests" => Body::List(pull_requests(&snapshot.pull_requests)),
        "services" => Body::List(services(not_yet(&snapshot.services)?)),
        "todos" => Body::List(todos(not_yet(&snapshot.todos)?)),
        _ => return Err(format!("zeron:{name} isn't a source the app has")),
    };
    Ok(Payload {
        updated: chrono::DateTime::from_timestamp_millis(snapshot.taken_at_ms)
            .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        errors: Vec::new(),
        body,
    })
}

fn not_yet<T>(part: &Option<T>) -> Result<&T, String> {
    part.as_ref()
        .ok_or_else(|| "not available in the app yet".to_string())
}

/// Elapsed time from `then_ms` to `now_ms`, like "4m".
fn age_ms(then_ms: i64, now_ms: i64) -> String {
    short_age_secs(now_ms.saturating_sub(then_ms) / 1000)
}

fn sessions(sessions: &[SessionItem], now_ms: i64) -> Vec<ListItem> {
    let mut live: Vec<(&SessionItem, &str)> = sessions
        .iter()
        .filter_map(|s| {
            let status = match s.status {
                SessionStatus::Working => "working",
                SessionStatus::AwaitingInput => "waiting",
                SessionStatus::Errored => "error",
                SessionStatus::Completed | SessionStatus::Idle => return None,
            };
            Some((s, status))
        })
        .collect();
    live.sort_by_key(|(s, _)| std::cmp::Reverse(s.last_activity_ms));
    live.into_iter()
        .map(|(s, status)| ListItem {
            id: Some(s.chat_id.clone()),
            title: s.title.clone(),
            sub: joined(&[
                s.harness.as_deref(),
                s.device_name.as_deref(),
                s.location.as_deref(),
            ]),
            age: s.started_at_ms.map(|started| age_ms(started, now_ms)),
            status: Some(status.to_string()),
            link: Some(chat_link(&s.chat_id)),
            ..ListItem::default()
        })
        .collect()
}

fn devices(devices: &[DeviceSnapshot], now_ms: i64) -> Vec<DeviceItem> {
    let mut sorted: Vec<&DeviceSnapshot> = devices.iter().collect();
    sorted.sort_by(|a, b| {
        b.is_self
            .cmp(&a.is_self)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    sorted
        .into_iter()
        .map(|d| DeviceItem {
            id: Some(d.id.clone()),
            name: d.name.clone(),
            platform: Some(d.platform.clone()).filter(|p| !p.is_empty()),
            online: d.online,
            role: None,
            detail: if d.working_sessions > 0 {
                let agents = if d.working_sessions == 1 {
                    "agent"
                } else {
                    "agents"
                };
                Some(format!("{} {agents} running", d.working_sessions))
            } else if !d.online {
                d.last_seen_ms
                    .map(|seen| format!("seen {} ago", age_ms(seen, now_ms)))
            } else {
                None
            },
            is_self: d.is_self,
        })
        .collect()
}

fn pull_requests(prs: &[PullRequestItem]) -> Vec<ListItem> {
    let mut seen = HashSet::new();
    let mut unique: Vec<&PullRequestItem> = prs
        .iter()
        .filter(|pr| seen.insert(pr.url.as_str()))
        .collect();
    unique.sort_by_key(|pr| pr.state != PrState::Open);
    unique
        .into_iter()
        .map(|pr| {
            let number = format!("{}#{}", pr.repo.as_deref().unwrap_or(""), pr.number);
            ListItem {
                id: Some(pr.url.clone()),
                title: pr.title.clone(),
                sub: joined(&[Some(number.as_str()), Some(pr.head_ref.as_str())]),
                badge: Some(
                    match pr.state {
                        PrState::Open => "open",
                        PrState::Merged => "merged",
                        PrState::Closed => "closed",
                    }
                    .to_string(),
                ),
                link: Some(pr.url.clone()),
                ..ListItem::default()
            }
        })
        .collect()
}

fn services(services: &[ServiceItem]) -> Vec<ListItem> {
    services
        .iter()
        .map(|s| ListItem {
            title: s.name.clone(),
            sub: joined(&[Some(s.project.as_str()), Some(&format!(":{}", s.port))]),
            link: Some(s.url.clone()),
            ..ListItem::default()
        })
        .collect()
}

fn todos(lists: &[TodoList]) -> Vec<ListItem> {
    lists
        .iter()
        .map(|list| {
            let done = list.items.iter().filter(|(_, done)| *done).count();
            let total = list.items.len();
            ListItem {
                id: Some(list.chat_id.clone()),
                title: list.title.clone(),
                sub: Some(format!("{done} of {total}")),
                status: Some(if done < total { "working" } else { "done" }.to_string()),
                link: Some(chat_link(&list.chat_id)),
                ..ListItem::default()
            }
        })
        .collect()
}

/// The non-blank parts joined with " · ", or `None` when all are blank.
fn joined(parts: &[Option<&str>]) -> Option<String> {
    let parts: Vec<&str> = parts
        .iter()
        .flatten()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_460_800_000;
    const MIN: i64 = 60_000;

    fn session(id: &str, status: SessionStatus, last_activity_ms: i64) -> SessionItem {
        SessionItem {
            chat_id: id.to_string(),
            title: format!("Chat {id}"),
            status,
            last_activity_ms,
            ..SessionItem::default()
        }
    }

    fn list(name: &str, snapshot: &ZeronSnapshot) -> Vec<ListItem> {
        match payload(name, snapshot, NOW).unwrap().body {
            Body::List(items) => items,
            _ => panic!("not a list"),
        }
    }

    #[test]
    fn sessions_show_live_chats_newest_first() {
        let mut working = session("w", SessionStatus::Working, NOW - 10 * MIN);
        working.harness = Some("Claude Code".into());
        working.location = Some("keron · main".into());
        working.started_at_ms = Some(NOW - 4 * MIN);
        let snapshot = ZeronSnapshot {
            sessions: vec![
                working,
                session("done", SessionStatus::Completed, NOW),
                session("idle", SessionStatus::Idle, NOW),
                session("wait", SessionStatus::AwaitingInput, NOW - MIN),
                session("err", SessionStatus::Errored, NOW - 20 * MIN),
            ],
            ..ZeronSnapshot::default()
        };
        let items = list("sessions", &snapshot);
        let rows: Vec<(&str, &str)> = items
            .iter()
            .map(|i| (i.title.as_str(), i.status.as_deref().unwrap()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Chat wait", "waiting"),
                ("Chat w", "working"),
                ("Chat err", "error")
            ]
        );
        let w = &items[1];
        assert_eq!(w.sub.as_deref(), Some("Claude Code · keron · main"));
        assert_eq!(w.age.as_deref(), Some("4m"));
        assert_eq!(w.link.as_deref(), Some("keron-home://chat/w"));
        assert_eq!(items[0].sub, None);
    }

    #[test]
    fn devices_put_this_mac_first_and_say_whats_running() {
        let device = |name: &str, online: bool, working: u32, is_self: bool| DeviceSnapshot {
            id: name.to_lowercase(),
            name: name.to_string(),
            platform: "macos".to_string(),
            online,
            last_seen_ms: Some(NOW - 3 * 60 * MIN),
            is_self,
            working_sessions: working,
        };
        let snapshot = ZeronSnapshot {
            devices: vec![
                device("mini", true, 0, false),
                device("iPhone", false, 0, false),
                device("MacBook", true, 2, true),
                device("Studio", true, 1, false),
            ],
            ..ZeronSnapshot::default()
        };
        let Body::Devices(items) = payload("devices", &snapshot, NOW).unwrap().body else {
            panic!("not devices")
        };
        let rows: Vec<(&str, Option<&str>)> = items
            .iter()
            .map(|d| (d.name.as_str(), d.detail.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("MacBook", Some("2 agents running")),
                ("iPhone", Some("seen 3h ago")),
                ("mini", None),
                ("Studio", Some("1 agent running")),
            ]
        );
    }

    #[test]
    fn pull_requests_are_deduplicated_and_open_first() {
        let pr = |chat: &str, number: u64, state: PrState| PullRequestItem {
            chat_id: chat.to_string(),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/acme/app/pull/{number}"),
            repo: Some("acme/app".to_string()),
            state,
            head_ref: format!("branch-{number}"),
        };
        let snapshot = ZeronSnapshot {
            pull_requests: vec![
                pr("a", 1, PrState::Merged),
                pr("b", 2, PrState::Open),
                pr("c", 2, PrState::Open),
            ],
            ..ZeronSnapshot::default()
        };
        let items = list("pull-requests", &snapshot);
        let rows: Vec<(&str, &str, &str)> = items
            .iter()
            .map(|i| {
                (
                    i.title.as_str(),
                    i.sub.as_deref().unwrap(),
                    i.badge.as_deref().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("PR 2", "acme/app#2 · branch-2", "open"),
                ("PR 1", "acme/app#1 · branch-1", "merged")
            ]
        );
    }

    #[test]
    fn sources_without_data_say_so() {
        let snapshot = ZeronSnapshot::default();
        assert_eq!(
            payload("services", &snapshot, NOW),
            Err("not available in the app yet".to_string())
        );
        assert_eq!(
            payload("todos", &snapshot, NOW),
            Err("not available in the app yet".to_string())
        );
        assert!(payload("weather", &snapshot, NOW).is_err());

        let snapshot = ZeronSnapshot {
            todos: Some(vec![TodoList {
                chat_id: "t".into(),
                title: "Refactor".into(),
                items: vec![("one".into(), true), ("two".into(), false)],
            }]),
            ..ZeronSnapshot::default()
        };
        let items = list("todos", &snapshot);
        assert_eq!(items[0].sub.as_deref(), Some("1 of 2"));
        assert_eq!(items[0].status.as_deref(), Some("working"));
    }
}
