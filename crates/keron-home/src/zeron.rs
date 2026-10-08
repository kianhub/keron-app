//! `zeron:<name>` sources: data the app already has, as a plain snapshot
//! the ui crate fills from its AppState (crates/ui/src/home/bridge.rs), so
//! this crate needs neither gpui nor the ui crate.
//!
//! Names: `sessions` (working and waiting chats), `devices`,
//! `pull-requests`, `services`, `todos`, `usage` (plan usage per agent
//! account). `services` and `todos` have no source in the app yet, and
//! `usage` is `None` until the app has loaded the accounts list; the payload
//! says so instead of showing an empty list.

use std::collections::HashSet;

use serde::Serialize;

use crate::kinds::{chat_link, short_age_secs};
use crate::{Body, DeviceItem, ListItem, Meter, Payload};

/// The names a `zeron:` source may use.
pub const NAMES: &[&str] = &[
    "sessions",
    "devices",
    "pull-requests",
    "services",
    "todos",
    "usage",
];

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
    /// The agent accounts signed in on this Mac and their plan usage, as
    /// Settings → Accounts and the composer's usage ring show them; `None`
    /// until the app has loaded that list.
    pub usage: Option<Vec<UsageAccount>>,
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

/// One agent account's plan usage. Plain data: the email is already cut
/// down to [`who`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UsageAccount {
    /// The provider as the owner calls it: "Claude", "ChatGPT", "Cursor"...
    pub harness: String,
    /// "Max", "Pro"..., when the provider says.
    pub plan: Option<String>,
    /// Which account, short: see [`who`].
    pub who: String,
    /// The login the agent uses now.
    pub active: bool,
    /// The plan's rate-limit windows, in the provider's order.
    pub windows: Vec<UsageWindow>,
    /// When the windows were fetched, ms since the epoch.
    pub fetched_at_ms: Option<i64>,
    /// Why the last usage probe failed, as the engine says it.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UsageWindow {
    /// "Session", "Week", "Day", "Month", as the provider names it.
    pub label: String,
    /// 0.0 to 1.0.
    pub used: f32,
    /// When it resets, ms since the epoch, when the provider says.
    pub resets_at_ms: Option<i64>,
}

/// An account's short name for Home: the part of its email before the @,
/// or "you". Never the whole address.
pub fn who(email: Option<&str>) -> String {
    email
        .and_then(|email| email.split('@').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("you")
        .to_string()
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
/// open ones first, deduplicated by URL. `usage` lists the accounts in use
/// and the others that report usage, the ones in use first, each with a
/// [`crate::Meter`] per window.
pub fn payload(name: &str, snapshot: &ZeronSnapshot, now_ms: i64) -> Result<Payload, String> {
    let body = match name {
        "sessions" => Body::List(sessions(&snapshot.sessions, now_ms)),
        "devices" => Body::Devices(devices(&snapshot.devices, now_ms)),
        "pull-requests" => Body::List(pull_requests(&snapshot.pull_requests)),
        "services" => Body::List(services(not_yet(&snapshot.services)?)),
        "todos" => Body::List(todos(not_yet(&snapshot.todos)?)),
        "usage" => Body::List(usage(not_yet(&snapshot.usage)?, now_ms)),
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

/// One row per account in use and per other account with usage windows,
/// the ones in use first, otherwise in the app's order. The title is
/// "Claude · Max"; the sub lists each window's use ("Session 42% · Week
/// 61%"), or, for an account in use with no windows, why ("Usage
/// unavailable" or the engine's reason). The row says which account when its
/// provider has more than one.
fn usage(accounts: &[UsageAccount], now_ms: i64) -> Vec<ListItem> {
    let mut shown: Vec<&UsageAccount> = accounts
        .iter()
        .filter(|account| account.active || !account.windows.is_empty())
        .collect();
    shown.sort_by_key(|account| !account.active);
    let shared = |harness: &str| {
        accounts
            .iter()
            .filter(|account| account.harness == harness)
            .count()
            > 1
    };
    shown
        .iter()
        .map(|account| {
            let used = |window: &UsageWindow| (window.used.clamp(0.0, 1.0) * 100.0).round() as u32;
            let sub = if account.windows.is_empty() {
                account
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|error| !error.is_empty())
                    .unwrap_or("Usage unavailable")
                    .to_string()
            } else {
                account
                    .windows
                    .iter()
                    .map(|window| format!("{} {}%", window.label.trim(), used(window)))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            ListItem {
                title: joined(&[Some(account.harness.as_str()), account.plan.as_deref()])
                    .unwrap_or_else(|| "Agent".to_string()),
                sub: Some(sub),
                badge: account.active.then(|| "in use".to_string()),
                age: account.fetched_at_ms.map(|at| age_ms(at, now_ms)),
                account: shared(&account.harness).then(|| account.who.clone()),
                meters: account
                    .windows
                    .iter()
                    .map(|window| Meter {
                        label: window.label.trim().to_string(),
                        used: window.used.clamp(0.0, 1.0),
                        resets_at_ms: window.resets_at_ms,
                    })
                    .collect(),
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

    fn account(harness: &str, plan: &str, email: &str, active: bool, used: &[f32]) -> UsageAccount {
        UsageAccount {
            harness: harness.to_string(),
            plan: Some(plan.to_string()).filter(|p| !p.is_empty()),
            who: who(Some(email)),
            active,
            windows: ["Session", "Week"]
                .iter()
                .zip(used)
                .map(|(label, used)| UsageWindow {
                    label: label.to_string(),
                    used: *used,
                    resets_at_ms: Some(NOW + 60 * MIN),
                })
                .collect(),
            fetched_at_ms: Some(NOW - 3 * MIN),
            error: None,
        }
    }

    #[test]
    fn usage_lists_accounts_in_use_first_with_their_windows() {
        let snapshot = ZeronSnapshot {
            usage: Some(vec![
                account("Claude", "Max", "kian.work@acme.com", false, &[0.1, 0.2]),
                account("ChatGPT", "Pro", "kian@example.com", true, &[0.42, 1.3]),
                account("Claude", "Max", "kian@example.com", true, &[0.81, 0.6]),
                account("Cursor", "", "kian@example.com", true, &[]),
                account("Cursor", "", "kian.work@acme.com", false, &[]),
            ]),
            ..ZeronSnapshot::default()
        };
        let items = list("usage", &snapshot);
        let rows: Vec<(&str, &str, Option<&str>, Option<&str>)> = items
            .iter()
            .map(|i| {
                (
                    i.title.as_str(),
                    i.sub.as_deref().unwrap(),
                    i.badge.as_deref(),
                    i.account.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "ChatGPT · Pro",
                    "Session 42% · Week 100%",
                    Some("in use"),
                    None
                ),
                (
                    "Claude · Max",
                    "Session 81% · Week 60%",
                    Some("in use"),
                    Some("kian")
                ),
                ("Cursor", "Usage unavailable", Some("in use"), Some("kian")),
                (
                    "Claude · Max",
                    "Session 10% · Week 20%",
                    None,
                    Some("kian.work")
                ),
            ]
        );
        assert!(items[2].meters.is_empty());
        assert_eq!(items[0].age.as_deref(), Some("3m"));
        let meters: Vec<(&str, f32)> = items[0]
            .meters
            .iter()
            .map(|m| (m.label.as_str(), m.used))
            .collect();
        assert_eq!(meters, [("Session", 0.42), ("Week", 1.0)]);
        assert_eq!(items[0].meters[0].resets_at_ms, Some(NOW + 60 * MIN));
    }

    #[test]
    fn usage_keeps_the_account_in_use_when_its_probe_failed() {
        let mut failed = account("Claude", "Max", "kian@example.com", true, &[]);
        failed.error = Some("Rate limited, retrying in 2m".to_string());
        failed.fetched_at_ms = None;
        let snapshot = ZeronSnapshot {
            usage: Some(vec![
                account("Claude", "Max", "kian.work@acme.com", false, &[0.1, 0.2]),
                failed,
            ]),
            ..ZeronSnapshot::default()
        };
        let items = list("usage", &snapshot);
        let rows: Vec<(&str, Option<&str>, Option<&str>)> = items
            .iter()
            .map(|i| {
                (
                    i.sub.as_deref().unwrap(),
                    i.badge.as_deref(),
                    i.account.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("Rate limited, retrying in 2m", Some("in use"), Some("kian")),
                ("Session 10% · Week 20%", None, Some("kian.work")),
            ]
        );
    }

    #[test]
    fn usage_never_shows_a_whole_email() {
        assert_eq!(who(Some("kian@example.com")), "kian");
        assert_eq!(who(Some("@example.com")), "you");
        assert_eq!(who(Some("  ")), "you");
        assert_eq!(who(None), "you");
    }

    #[test]
    fn usage_says_when_the_app_has_no_accounts_list_yet() {
        let snapshot = ZeronSnapshot::default();
        assert_eq!(
            payload("usage", &snapshot, NOW),
            Err("not available in the app yet".to_string())
        );
        let snapshot = ZeronSnapshot {
            usage: Some(Vec::new()),
            ..ZeronSnapshot::default()
        };
        assert!(list("usage", &snapshot).is_empty());
    }
}
