//! Owner actions on loose ends, through the door:
//! `POST /memory/loose-ends/{snooze|done|dismiss|shown}` with
//! `{"ids": [...], "until": "..."}`. The server changes heat and writes the
//! notes (`[mini/loose-ends] Snoozed: ... until 14:00`, `Done: ...`).

use chrono::{DateTime, Local, Timelike};
use keron_door::DoorClient;
use serde_json::{Value, json};

use crate::kinds::parse_payload;
use crate::{Body, FetchError, Kind, ListItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Until a time the server's `parse_until` reads: `+2h`, `18:00`,
    /// `tomorrow`, or ISO 8601.
    Snooze {
        until: String,
    },
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
        match self {
            Action::Snooze { .. } => "snooze",
            Action::Done => "done",
            Action::Dismiss => "dismiss",
            Action::Shown => "shown",
        }
    }
}

/// Post the action; returns the changed items, in the list shape.
pub async fn act(
    door: &DoorClient,
    action: &Action,
    ids: &[String],
) -> Result<Vec<ListItem>, FetchError> {
    let answer = door
        .post_json(
            &format!("/memory/loose-ends/{}", action.name()),
            &body(action, ids),
        )
        .await?;
    match parse_payload(Kind::List, &answer)
        .map_err(FetchError::Payload)?
        .body
    {
        Body::List(items) => Ok(items),
        _ => Ok(Vec::new()),
    }
}

/// `{"ids": [...]}`, plus `"until"` for a snooze.
fn body(action: &Action, ids: &[String]) -> Value {
    let mut body = json!({ "ids": ids });
    if let Action::Snooze { until } = action {
        body["until"] = Value::String(until.clone());
    }
    body
}

/// Snooze choices for a menu at `now`, as `(label, until)`: "1 hour"
/// (`+1h`), "This evening" (`18:00`, only before 17:00), "Tomorrow"
/// (`tomorrow`, 9:00), "Next week" (`+7d`).
pub fn snooze_choices(now: DateTime<Local>) -> Vec<(&'static str, String)> {
    let mut choices = vec![("1 hour", "+1h".to_string())];
    if now.hour() < 17 {
        choices.push(("This evening", "18:00".to_string()));
    }
    choices.push(("Tomorrow", "tomorrow".to_string()));
    choices.push(("Next week", "+7d".to_string()));
    choices
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn evening_is_offered_only_before_five() {
        let at = |hour| {
            Local
                .with_ymd_and_hms(2026, 10, 8, hour, 30, 0)
                .single()
                .unwrap()
        };
        let labels = |hour| {
            snooze_choices(at(hour))
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(16),
            ["1 hour", "This evening", "Tomorrow", "Next week"]
        );
        assert_eq!(labels(17), ["1 hour", "Tomorrow", "Next week"]);
    }

    #[test]
    fn only_snooze_sends_until() {
        let ids = ["a1b2c3".to_string()];
        let snooze = Action::Snooze {
            until: "+1h".into(),
        };
        assert_eq!(
            body(&snooze, &ids),
            json!({"ids": ["a1b2c3"], "until": "+1h"})
        );
        assert_eq!(body(&Action::Done, &ids), json!({"ids": ["a1b2c3"]}));
    }
}
