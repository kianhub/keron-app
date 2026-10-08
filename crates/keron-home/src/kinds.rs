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
    pub const ALL: [Kind; 5] = [
        Kind::List,
        Kind::Timeline,
        Kind::Stat,
        Kind::Agenda,
        Kind::Devices,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        Kind::ALL
            .into_iter()
            .find(|kind| kind.name() == text.trim())
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::List => "list",
            Kind::Timeline => "timeline",
            Kind::Stat => "stat",
            Kind::Agenda => "agenda",
            Kind::Devices => "devices",
        }
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
        match self {
            Body::List(_) => Kind::List,
            Body::Timeline(_) => Kind::Timeline,
            Body::Stat(_) => Kind::Stat,
            Body::Agenda(_) => Kind::Agenda,
            Body::Devices(_) => Kind::Devices,
        }
    }

    /// Rows (or 1 for a stat with a value), for the card's count.
    pub fn len(&self) -> usize {
        match self {
            Body::List(items) => items.len(),
            Body::Timeline(items) => items.len(),
            Body::Stat(stat) => usize::from(!stat.value.is_empty()),
            Body::Agenda(items) => items.len(),
            Body::Devices(items) => items.len(),
        }
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
    /// Loose ends: the heat level (3 hot, 4 burning) a notification is due
    /// for right now, as the server decides; `None` when none is due.
    pub notify: Option<u8>,
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
    /// Usage meters under the title, drawn instead of `sub`. Only the app's
    /// own `zeron:usage` source sets them; they aren't part of the JSON a
    /// script or the door sends.
    #[serde(skip)]
    pub meters: Vec<Meter>,
}

/// One usage meter on a list row: a plan's rate-limit window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meter {
    /// The window, as the provider names it ("Session", "Week").
    pub label: String,
    /// How much of it is used, 0.0 to 1.0.
    pub used: f32,
    /// When it resets, ms since the epoch, when the provider says.
    pub resets_at_ms: Option<i64>,
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
///
/// Lenient on purpose: a field of the wrong type counts as missing, numbers
/// may arrive as floats, heat outside 0..=4 is clamped, and a stat's value
/// may be a number or a string. A row fails only when it isn't an object or
/// lacks its main text (`title`, `text` or `name`; an agenda row also needs
/// `start`).
pub fn parse_payload(kind: Kind, value: &Value) -> Result<Payload, PayloadError> {
    let Some(object) = value.as_object() else {
        return Err(PayloadError::Shape(
            "the payload isn't a JSON object".to_string(),
        ));
    };
    let updated = object.get("updated").and_then(text);
    let mut errors: Vec<String> = match object.get("errors") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|e| text(e).or_else(|| (!e.is_null()).then(|| e.to_string())))
            .collect(),
        Some(other) => text(other).into_iter().collect(),
        None => Vec::new(),
    };
    let body = match kind {
        Kind::Stat => Body::Stat(stat(object)),
        Kind::List => Body::List(rows(object, &mut errors, list_item)?),
        Kind::Timeline => Body::Timeline(rows(object, &mut errors, timeline_item)?),
        Kind::Agenda => Body::Agenda(rows(object, &mut errors, agenda_item)?),
        Kind::Devices => Body::Devices(rows(object, &mut errors, device_item)?),
    };
    Ok(Payload {
        updated,
        errors,
        body,
    })
}

type Object = serde_json::Map<String, Value>;

fn rows<T>(
    object: &Object,
    errors: &mut Vec<String>,
    parse: fn(&Object) -> Option<T>,
) -> Result<Vec<T>, PayloadError> {
    let Some(Value::Array(items)) = object.get("items") else {
        return Err(PayloadError::Shape(
            "the payload has no \"items\" list".to_string(),
        ));
    };
    let parsed: Vec<T> = items
        .iter()
        .filter_map(|item| item.as_object().and_then(parse))
        .collect();
    let skipped = items.len() - parsed.len();
    if skipped > 0 {
        let rows = if skipped == 1 { "row" } else { "rows" };
        errors.push(format!("{skipped} {rows} didn't parse and were skipped"));
    }
    Ok(parsed)
}

fn list_item(o: &Object) -> Option<ListItem> {
    Some(ListItem {
        id: field(o, "id"),
        title: field(o, "title")?,
        sub: field(o, "sub"),
        badge: field(o, "badge"),
        age: field(o, "age"),
        at: field(o, "at"),
        heat: o
            .get("heat")
            .and_then(number)
            .map(|h| h.round().clamp(0.0, 4.0) as u8),
        notify: o
            .get("notify")
            .and_then(number)
            .map(|n| n.round())
            .filter(|n| (3.0..=4.0).contains(n))
            .map(|n| n as u8),
        link: field(o, "link"),
        account: field(o, "account"),
        status: field(o, "status"),
        actions: match o.get("actions") {
            Some(Value::Array(list)) => list.iter().filter_map(text).collect(),
            _ => Vec::new(),
        },
        kind: field(o, "kind"),
        snoozed_until: field(o, "snoozed_until"),
        deadline: field(o, "deadline"),
        meters: Vec::new(),
    })
}

fn timeline_item(o: &Object) -> Option<TimelineItem> {
    Some(TimelineItem {
        time: field(o, "time"),
        at: field(o, "at"),
        kind: field(o, "kind"),
        text: field(o, "text")?,
        tag: field(o, "tag"),
        n: o.get("n")
            .and_then(number)
            .filter(|n| *n >= 0.0)
            .map(|n| n as u64),
    })
}

fn agenda_item(o: &Object) -> Option<AgendaItem> {
    Some(AgendaItem {
        id: field(o, "id"),
        title: field(o, "title")?,
        start: field(o, "start")?,
        end: field(o, "end"),
        all_day: o.get("all_day").and_then(Value::as_bool).unwrap_or(false),
        attendees: o
            .get("attendees")
            .and_then(number)
            .filter(|n| *n >= 0.0)
            .map(|n| n.min(f64::from(u32::MAX)) as u32),
        video_link: field(o, "video_link"),
        link: field(o, "link"),
        account: field(o, "account"),
        calendar: field(o, "calendar"),
        warn: field(o, "warn"),
    })
}

fn device_item(o: &Object) -> Option<DeviceItem> {
    Some(DeviceItem {
        id: field(o, "id"),
        name: field(o, "name")?,
        platform: field(o, "platform"),
        online: o.get("online").and_then(Value::as_bool).unwrap_or(false),
        role: field(o, "role"),
        detail: field(o, "detail"),
        is_self: o.get("is_self").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn stat(o: &Object) -> Stat {
    let value = match o.get("value") {
        Some(Value::Number(n)) => number_text(n),
        Some(other) => text(other).unwrap_or_default(),
        None => String::new(),
    };
    Stat {
        value,
        label: field(o, "label"),
        series: match o.get("series") {
            Some(Value::Array(list)) => list.iter().filter_map(number).collect(),
            _ => Vec::new(),
        },
        delta: field(o, "delta"),
    }
}

/// A string, or a number written out (ids sometimes come as numbers).
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(number_text(n)),
        _ => None,
    }
}

/// A text field; blank strings count as missing.
fn field(o: &Object, key: &str) -> Option<String> {
    o.get(key).and_then(text).filter(|s| !s.trim().is_empty())
}

/// A number, or a string holding one.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// `120`, not `120.0`; `4.2` stays `4.2`.
fn number_text(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

/// Parse an ISO 8601 date-time (with offset or `Z`).
pub fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text.trim()).ok()
}

/// Parse a `YYYY-MM-DD` date (all-day events).
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// A short age like the door's: "45s", "12m", "3h", "2d", "5w".
pub fn short_age(then: DateTime<FixedOffset>, now: DateTime<chrono::Utc>) -> String {
    short_age_secs(now.signed_duration_since(then).num_seconds())
}

/// [`short_age`] from elapsed seconds (negative counts as 0). The same
/// thresholds as keron-sources' `age()`.
pub fn short_age_secs(secs: i64) -> String {
    let secs = secs.max(0);
    for (unit, size) in [("w", 604_800), ("d", 86_400), ("h", 3_600), ("m", 60)] {
        if secs >= size {
            return format!("{}{unit}", secs / size);
        }
    }
    format!("{secs}s")
}

const CHAT_LINK_PREFIX: &str = "keron-home://chat/";

/// The internal link that opens one of the app's own chats.
pub fn chat_link(chat_id: &str) -> String {
    format!("{CHAT_LINK_PREFIX}{chat_id}")
}

/// The chat id from a [`chat_link`], if it is one.
pub fn parse_chat_link(link: &str) -> Option<&str> {
    link.strip_prefix(CHAT_LINK_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Whether a widget-supplied link may be opened in the browser: http and
/// https only.
pub fn is_openable(link: &str) -> bool {
    if link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = link.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if host_port.starts_with('[') {
        host_port
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    !host.is_empty()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keron_sources_list_parses_as_served_and_skips_bad_rows() {
        let served = json!({
            "source": "slack-waiting",
            "kind": "list",
            "updated": "2026-10-08T09:30:00Z",
            "items": [
                {"title": "Can you look at the draft?", "sub": "#design · Sam", "badge": "dm",
                 "at": "2026-10-08T09:00:00Z", "age": "30m", "link": "https://example.slack.com/archives/C1/p1",
                 "account": "acme", "heat": null, "id": "C1-1"},
                {"title": "Old thread", "heat": 2.7, "id": 42, "notify": 3},
                {"title": "Way too hot", "heat": 9, "notify": 2},
                {"title": "Below zero", "heat": -1},
                {"sub": "no title"},
                {"title": "   "},
                "not an object"
            ],
            "errors": ["personal: token expired"]
        });
        let payload = parse_payload(Kind::List, &served).unwrap();
        assert_eq!(payload.updated.as_deref(), Some("2026-10-08T09:30:00Z"));
        let Body::List(items) = &payload.body else {
            panic!("not a list")
        };
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].account.as_deref(), Some("acme"));
        assert_eq!(items[0].age.as_deref(), Some("30m"));
        assert_eq!(items[0].heat, None);
        assert_eq!(items[1].id.as_deref(), Some("42"));
        assert_eq!(items[1].heat, Some(3));
        assert_eq!(items[1].notify, Some(3));
        assert_eq!(items[2].heat, Some(4));
        assert_eq!(items[2].notify, None, "only hot and burning notify");
        assert_eq!(items[3].heat, Some(0));
        assert_eq!(
            payload.errors,
            [
                "personal: token expired",
                "3 rows didn't parse and were skipped"
            ]
        );

        assert!(matches!(
            parse_payload(Kind::List, &json!({"source": "x"})),
            Err(PayloadError::Shape(_))
        ));
        assert!(matches!(
            parse_payload(Kind::List, &json!([])),
            Err(PayloadError::Shape(_))
        ));
    }

    #[test]
    fn keron_sources_agenda_parses_as_served() {
        let served = json!({
            "source": "calendar-today", "kind": "agenda", "updated": "2026-10-08T06:00:00Z",
            "items": [
                {"title": "Design review", "start": "2026-10-08T15:00:00+01:00",
                 "end": "2026-10-08T15:30:00+01:00", "all_day": false, "attendees": 4.0,
                 "video_link": "https://meet.example.com/abc", "link": "https://calendar.example.com/e/1",
                 "account": "work", "calendar": "Team", "id": "ev1"},
                {"title": "Holiday", "start": "2026-10-08", "end": "2026-10-09", "all_day": true,
                 "attendees": null, "video_link": null, "link": null, "account": "personal",
                 "calendar": "Home", "id": "ev2"},
                {"title": "No start"}
            ],
            "errors": []
        });
        let payload = parse_payload(Kind::Agenda, &served).unwrap();
        let Body::Agenda(items) = &payload.body else {
            panic!("not an agenda")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].attendees, Some(4));
        assert!(parse_time(&items[0].start).is_some());
        assert!(items[1].all_day);
        assert_eq!(
            parse_date(&items[1].start),
            NaiveDate::from_ymd_opt(2026, 10, 8)
        );
        assert_eq!(payload.errors.len(), 1);
    }

    #[test]
    fn stat_value_may_be_a_number_or_a_string() {
        let stat = |v: Value| match parse_payload(Kind::Stat, &v).unwrap().body {
            Body::Stat(stat) => stat,
            _ => panic!("not a stat"),
        };
        assert_eq!(stat(json!({"value": 120.0})).value, "120");
        assert_eq!(stat(json!({"value": 4.25})).value, "4.25");
        let s = stat(json!({"value": "4.2k", "series": [1, "2", 3.5, null]}));
        assert_eq!(s.value, "4.2k");
        assert_eq!(s.series, [1.0, 2.0, 3.5]);
        assert!(
            parse_payload(Kind::Stat, &json!({}))
                .unwrap()
                .body
                .is_empty()
        );
    }

    #[test]
    fn short_age_matches_keron_sources() {
        let now = DateTime::parse_from_rfc3339("2026-10-08T12:00:00Z")
            .unwrap()
            .to_utc();
        let ago =
            |secs: i64| short_age((now - chrono::Duration::seconds(secs)).fixed_offset(), now);
        assert_eq!(ago(-30), "0s");
        assert_eq!(ago(59), "59s");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(3599), "59m");
        assert_eq!(ago(3600), "1h");
        assert_eq!(ago(86_399), "23h");
        assert_eq!(ago(86_400), "1d");
        assert_eq!(ago(604_799), "6d");
        assert_eq!(ago(604_800 * 5), "5w");
    }

    #[test]
    fn only_http_links_open() {
        assert!(is_openable("https://example.com/a?b=c"));
        assert!(is_openable("HTTP://example.com"));
        assert!(is_openable("https://user@example.com:8443/x"));
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "https://example.com/\n",
            "keron-home://chat/abc",
        ] {
            assert!(!is_openable(bad), "{bad}");
        }
        assert_eq!(parse_chat_link(&chat_link("c-1")), Some("c-1"));
        assert_eq!(parse_chat_link("keron-home://chat/"), None);
    }
}
