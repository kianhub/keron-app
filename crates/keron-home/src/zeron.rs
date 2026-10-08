//! `zeron:<name>` sources: data the app already has, as a plain snapshot
//! the ui crate fills from its AppState (crates/ui/src/home/bridge.rs), so
//! this crate needs neither gpui nor the ui crate.
//!
//! Names: `sessions` (working and waiting chats), `devices`,
//! `pull-requests`, `services`, `todos`. `services` and `todos` have no
//! source in the app yet; their snapshot fields are `None` and the payload
//! says so instead of showing an empty list.

use serde::Serialize;

use crate::Payload;

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
    let _ = (name, snapshot, now_ms);
    todo!("keron-home: zeron::payload")
}
