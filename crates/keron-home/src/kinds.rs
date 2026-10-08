//! The widget kinds and the JSON each one takes.
//!
//! Every payload is an object. `updated` (ISO 8601, when the data was
//! fetched or polled) and `errors` (short strings, for example
//! `"work: token expired"`) are optional on every kind. List, timeline,
//! agenda and devices put their rows in `items`; stat puts `value`, `label`
//! and `series` at the top level. Unknown fields are ignored and every
//! field but the row's main text is optional, so keron-sources' payloads
//! (`{"source","kind","updated","items","errors"}`) parse as they are.
//! Times are kept as the strings they came as; [`parse_time`] reads them.

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    List,
    Timeline,
    Stat,
    Agenda,
    Devices,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::List, Kind::Timeline, Kind::Stat, Kind::Agenda, Kind::Devices];

    pub fn parse(text: &str) -> Option<Self> {
        let _ = text;
        todo!("keron-home: Kind::parse")
    }

    pub fn name(self) -> &'static str {
        todo!("keron-home: Kind::name")
    }
}

/// One parsed payload.
#[derive(Clone, Debug, PartialEq)]
pub struct Payload {
    /// When the data was fetched or polled, as given (ISO 8601).
    pub updated: Option<String>,
    /// Per-account or per-part problems; the rows still show.
    pub errors: Vec<String>,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    List(Vec<ListItem>),
    Timeline(Vec<TimelineItem>),
    Stat(Stat),
    Agenda(Vec<AgendaItem>),
    Devices(Vec<DeviceItem>),
}

impl Body {
    pub fn kind(&self) -> Kind {
        todo!("keron-home: Body::kind")
    }

    /// Rows (or 1 for a stat with a value), for the card's count.
    pub fn len(&self) -> usize {
        todo!("keron-home: Body::len")
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A `list` row: Gmail, Slack, loose ends, sessions, pull requests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListItem {
    /// Stable id (loose ends need it for snooze and done).
    pub id: Option<String>,
    pub title: String,
    /// Second line.
    pub sub: Option<String>,
    /// A short pill, like "hot", "work", "draft".
    pub badge: Option<String>,
    /// A short age ("12m", "3h"), as served.
    pub age: Option<String>,
    /// When it happened (ISO 8601); used for an age when `age` is missing.
    pub at: Option<String>,
    /// Loose-ends heat, 0 to 4.
    pub heat: Option<u8>,
    /// Opened on click: http(s) only, or an internal link from [`chat_link`].
    pub link: Option<String>,
    /// Which account a row belongs to ("personal", "work", a workspace name).
    pub account: Option<String>,
    /// For sessions: "working", "waiting", "error", "done" or "idle".
    pub status: Option<String>,
    /// Owner actions this row offers: "snooze", "done", "dismiss".
    pub actions: Vec<String>,
    /// Loose ends: kind ("promise", "half-done", "waiting-on-you", "stale").
    pub kind: Option<String>,
    pub snoozed_until: Option<String>,
    pub deadline: Option<String>,
}

/// A `timeline` row: Memory · today.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineItem {
    /// "HH:MM" local time, when known.
    pub time: Option<String>,
    pub at: Option<String>,
    /// "did", "said", "heard", "agent" or "nudge".
    pub kind: Option<String>,
    pub text: String,
    /// The note's source tag, like "mbp/capture".
    pub tag: Option<String>,
    /// The note's number in the memory.
    pub n: Option<u64>,
}

/// A `stat`: one big number, a label, an optional bar series.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stat {
    /// Shown as given ("120", "4.2k").
    pub value: String,
    pub label: Option<String>,
    /// Bars, oldest first; the last one is "now".
    pub series: Vec<f64>,
    /// A short change, like "+12%".
    pub delta: Option<String>,
}

/// An `agenda` row: Today (keron-sources' calendar-today items parse as they are).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgendaItem {
    pub id: Option<String>,
    pub title: String,
    /// ISO 8601 date-time, or YYYY-MM-DD for all-day events.
    pub start: String,
    pub end: Option<String>,
    pub all_day: bool,
    pub attendees: Option<u32>,
    pub video_link: Option<String>,
    pub link: Option<String>,
    pub account: Option<String>,
    pub calendar: Option<String>,
    /// A short warning chip, like "deck not sent".
    pub warn: Option<String>,
}

/// A `devices` row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceItem {
    pub id: Option<String>,
    pub name: String,
    /// "macos", "ios"...
    pub platform: Option<String>,
    pub online: bool,
    /// "memory server", "capturing"...
    pub role: Option<String>,
    /// Right-hand detail, like "4,214 notes" or "1 agent running".
    pub detail: Option<String>,
    /// This Mac.
    pub is_self: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    #[error("the source didn't print JSON: {0}")]
    NotJson(String),
    #[error("{0}")]
    Shape(String),
}

/// Parse `value` as a payload of `kind`. Rows that don't parse are skipped
/// (counted into `errors` as one line); a payload that isn't an object, or
/// a list kind without an `items` array, is an error.
pub fn parse_payload(kind: Kind, value: &Value) -> Result<Payload, PayloadError> {
    let _ = (kind, value);
    todo!("keron-home: parse_payload")
}

/// Parse an ISO 8601 date-time (with offset or `Z`).
pub fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    let _ = text;
    todo!("keron-home: parse_time")
}

/// Parse a `YYYY-MM-DD` date (all-day events).
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let _ = text;
    todo!("keron-home: parse_date")
}

/// A short age like the door's: "45s", "12m", "3h", "2d", "5w".
pub fn short_age(then: DateTime<FixedOffset>, now: DateTime<chrono::Utc>) -> String {
    let _ = (then, now);
    todo!("keron-home: short_age")
}

/// The internal link that opens one of the app's own chats.
pub fn chat_link(chat_id: &str) -> String {
    let _ = chat_id;
    todo!("keron-home: chat_link")
}

/// The chat id from a [`chat_link`], if it is one.
pub fn parse_chat_link(link: &str) -> Option<&str> {
    let _ = link;
    todo!("keron-home: parse_chat_link")
}

/// Whether a widget-supplied link may be opened in the browser: http and
/// https only.
pub fn is_openable(link: &str) -> bool {
    let _ = link;
    todo!("keron-home: is_openable")
}
