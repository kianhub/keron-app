//! Owner actions on loose ends, through the door:
//! `POST /memory/loose-ends/{snooze|done|dismiss|shown}` with
//! `{"ids": [...], "until": "..."}`. The server changes heat and writes the
//! notes (`[mini/loose-ends] Snoozed: ... until 14:00`, `Done: ...`).

use chrono::{DateTime, Local};
use keron_door::DoorClient;

use crate::{FetchError, ListItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Until a time the server's `parse_until` reads: `+2h`, `18:00`,
    /// `tomorrow`, or ISO 8601.
    Snooze { until: String },
    Done,
    /// "Not a thing".
    Dismiss,
    /// The items were on screen; the server counts it at most once per item
    /// every few hours.
    Shown,
}

impl Action {
    /// The path segment: "snooze", "done", "dismiss", "shown".
    pub fn name(&self) -> &'static str {
        todo!("keron-home: Action::name")
    }
}

/// Post the action; returns the changed items, in the list shape.
pub async fn act(door: &DoorClient, action: &Action, ids: &[String]) -> Result<Vec<ListItem>, FetchError> {
    let _ = (door, action, ids);
    todo!("keron-home: loose_ends::act")
}

/// Snooze choices for a menu at `now`, as `(label, until)`: "1 hour"
/// (`+1h`), "This evening" (`18:00`, only before 17:00), "Tomorrow"
/// (`tomorrow`, 9:00), "Next week" (`+7d`).
pub fn snooze_choices(now: DateTime<Local>) -> Vec<(&'static str, String)> {
    let _ = now;
    todo!("keron-home: snooze_choices")
}
