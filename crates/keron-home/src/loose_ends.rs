//! Owner actions on a widget's rows, through the door: an action posts to
//! the widget's own source, `<door path>/<action>` with `{"ids": [...]}`.
//!
//! - Loose ends: `POST /memory/loose-ends/{snooze|done|dismiss|shown|notified}`,
//!   plus `"until"` for a snooze or `"level"` for notified. The server
//!   changes heat and writes the notes (`[mini/loose-ends] Snoozed: ...
//!   until 14:00`, `Done: ...`); notified writes none.
//! - keron-sources: `POST /sources/<name>/done` (gmail-needs-reply,
//!   slack-waiting) marks rows as needing no reply; the door leaves them out
//!   until a newer message comes. No note is written.

use chrono::{DateTime, Local, Timelike};
use keron_door::DoorClient;
use serde_json::{Value, json};

use crate::kinds::parse_payload;
use crate::{Body, FetchError, Kind, ListItem, SourceSpec};

/// The loose-ends source, `memory:loose-ends`.
pub fn loose_ends() -> SourceSpec {
    SourceSpec::Memory("loose-ends".to_string())
}

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
    /// A notification for heat `level` (3 hot, 4 burning) went out for the
    /// items; the server records it so it isn't due again.
    Notified {
        level: u8,
    },
}

impl Action {
    /// The path segment: "snooze", "done", "dismiss", "shown", "notified".
    pub fn name(&self) -> &'static str {
        match self {
            Action::Snooze { .. } => "snooze",
            Action::Done => "done",
            Action::Dismiss => "dismiss",
            Action::Shown => "shown",
            Action::Notified { .. } => "notified",
        }
    }
}

/// One action as the door takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// `<the source's door path>/<action>`, like `/sources/slack-waiting/done`.
    pub path: String,
    pub body: Value,
}

/// The request for `action` on `source`'s rows `ids`. Only door sources
/// (`memory:`, `keron-sources:`) take actions.
pub fn request(
    source: &SourceSpec,
    action: &Action,
    ids: &[String],
) -> Result<Request, FetchError> {
    let base = source
        .door_path()
        .ok_or_else(|| FetchError::Unsupported(format!("{source} rows don't take actions")))?;
    Ok(Request {
        path: format!("{base}/{}", action.name()),
        body: body(action, ids),
    })
}

/// Post the action to `source`; returns the changed items, in the list shape.
pub async fn act(
    door: &DoorClient,
    source: &SourceSpec,
    action: &Action,
    ids: &[String],
) -> Result<Vec<ListItem>, FetchError> {
    send(door, &request(source, action, ids)?).await
}

/// Post a request built by [`request`]; returns the changed items.
pub async fn send(door: &DoorClient, request: &Request) -> Result<Vec<ListItem>, FetchError> {
    let answer = door.post_json(&request.path, &request.body).await?;
    match parse_payload(Kind::List, &answer)
        .map_err(FetchError::Payload)?
        .body
    {
        Body::List(items) => Ok(items),
        _ => Ok(Vec::new()),
    }
}

/// `{"ids": [...]}`, plus `"until"` for a snooze and `"level"` for notified.
fn body(action: &Action, ids: &[String]) -> Value {
    let mut body = json!({ "ids": ids });
    match action {
        Action::Snooze { until } => body["until"] = Value::String(until.clone()),
        Action::Notified { level } => body["level"] = json!(level),
        Action::Done | Action::Dismiss | Action::Shown => {}
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
    fn an_action_posts_to_its_widgets_source() {
        let ids = ["C024BE91L:1728300000.000100".to_string()];
        let slack = SourceSpec::KeronSources("slack-waiting".to_string());
        assert_eq!(
            request(&slack, &Action::Done, &ids).unwrap(),
            Request {
                path: "/sources/slack-waiting/done".to_string(),
                body: json!({"ids": ["C024BE91L:1728300000.000100"]}),
            }
        );
        let level = Action::Notified { level: 3 };
        let notified = request(&loose_ends(), &level, &["a1".to_string()]).unwrap();
        assert_eq!(notified.path, "/memory/loose-ends/notified");
        assert_eq!(notified.body, json!({"ids": ["a1"], "level": 3}));
        let script = SourceSpec::Script("mine.sh".into());
        assert!(request(&script, &Action::Done, &ids).is_err());
    }

    #[test]
    fn snooze_sends_until_and_notified_sends_its_level() {
        let ids = ["a1b2c3".to_string()];
        let snooze = Action::Snooze {
            until: "+1h".into(),
        };
        assert_eq!(
            body(&snooze, &ids),
            json!({"ids": ["a1b2c3"], "until": "+1h"})
        );
        assert_eq!(body(&Action::Done, &ids), json!({"ids": ["a1b2c3"]}));
        let notified = Action::Notified { level: 4 };
        assert_eq!(notified.name(), "notified");
        let two = ["a1".to_string(), "b2".to_string()];
        assert_eq!(
            body(&notified, &two),
            json!({"ids": ["a1", "b2"], "level": 4})
        );
    }
}
