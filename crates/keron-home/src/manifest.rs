//! One widget's manifest: `~/.keron/widgets/<id>.toml`.
//!
//! ```toml
//! title  = "Slack · waiting on you"
//! kind   = "list"            # list, timeline, stat, agenda, devices
//! source = "keron-sources:slack-waiting"
//! every  = "1m"              # optional, default 1m, from 5s to 24h
//! icon   = "chat"            # optional, see ICONS
//! wide   = false             # optional: two columns until the layout says otherwise
//! limit  = 8                 # optional: most rows shown
//! empty  = "Nothing waiting" # optional: text when there are no rows
//! ```
//!
//! The id is the file name without `.toml`. Unknown keys are an error, so a
//! typo shows up instead of being ignored.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::{Kind, SourceSpec, zeron};

/// Icon names a manifest may use; the app draws each one.
pub const ICONS: &[&str] = &[
    "flag", "chat", "check", "calendar", "tree", "pulse", "devices", "pr", "mail", "mic", "bars",
    "widget", "bell", "star", "list", "globe",
];

/// Default refresh.
pub const DEFAULT_EVERY: Duration = Duration::from_secs(60);
/// Fastest refresh a manifest may ask for.
pub const MIN_EVERY: Duration = Duration::from_secs(5);
/// Slowest refresh a manifest may ask for. Timers and staleness math stay
/// well inside their range.
pub const MAX_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub source: SourceSpec,
    pub every: Duration,
    pub icon: Option<String>,
    pub wide: bool,
    pub limit: Option<usize>,
    pub empty: Option<String>,
    /// The file it came from; `None` for a built-in.
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {message}")]
pub struct ManifestError {
    /// The file name (or id), never the full path's parents.
    pub file: String,
    pub message: String,
}

/// Whether `id` can name a widget: `[a-z0-9][a-z0-9_-]{0,47}`.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 48
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Longest title.
const MAX_TITLE: usize = 80;
/// Most rows a manifest may ask for.
const MAX_LIMIT: i64 = 50;

/// The file as written; checked into a [`Manifest`] by [`parse`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    title: String,
    kind: String,
    source: String,
    every: Option<RawEvery>,
    icon: Option<String>,
    wide: Option<bool>,
    limit: Option<i64>,
    empty: Option<String>,
}

/// `every = "30s"` or `every = 30`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawEvery {
    Text(String),
    Seconds(i64),
}

/// Parse a manifest's TOML. `id` is the file stem.
pub fn parse(id: &str, text: &str) -> Result<Manifest, ManifestError> {
    let fail = |message: String| ManifestError {
        file: format!("{id}.toml"),
        message,
    };
    if !valid_id(id) {
        return Err(fail(
            "the file name isn't a valid widget id (use lowercase letters, digits, - and _, \
             starting with a letter or digit, at most 48 characters)"
                .to_string(),
        ));
    }
    let raw: RawManifest = toml::from_str(text).map_err(|e| fail(toml_problem(&e, text)))?;

    let title = raw.title.trim().to_string();
    if title.is_empty() {
        return Err(fail("title is empty".to_string()));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(fail(format!("title is longer than {MAX_TITLE} characters")));
    }
    let kind = Kind::parse(&raw.kind).ok_or_else(|| {
        let names: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
        fail(format!(
            "kind \"{}\" isn't one of {}",
            raw.kind,
            names.join(", ")
        ))
    })?;
    let source = SourceSpec::parse(&raw.source).map_err(fail)?;
    if let SourceSpec::Zeron(name) = &source
        && !zeron::NAMES.contains(&name.as_str())
    {
        return Err(fail(format!(
            "zeron:{name} isn't a source the app has; use one of {}",
            zeron::NAMES.join(", ")
        )));
    }
    let every = match raw.every {
        None => DEFAULT_EVERY,
        Some(every) => {
            let parsed = match &every {
                RawEvery::Text(text) => parse_every(text),
                RawEvery::Seconds(secs) => u64::try_from(*secs).ok().map(Duration::from_secs),
            };
            let shown = match &every {
                RawEvery::Text(text) => text.clone(),
                RawEvery::Seconds(secs) => secs.to_string(),
            };
            let every = parsed.ok_or_else(|| {
                fail(format!(
                    "every \"{shown}\" isn't a duration; write it like 30s, 1m or 1h"
                ))
            })?;
            if every < MIN_EVERY {
                return Err(fail(format!(
                    "every \"{shown}\" is too often; the fastest is {}s",
                    MIN_EVERY.as_secs()
                )));
            }
            if every > MAX_EVERY {
                return Err(fail(format!(
                    "every \"{shown}\" is too long; the slowest is 24h"
                )));
            }
            every
        }
    };
    if let Some(icon) = &raw.icon
        && !ICONS.contains(&icon.as_str())
    {
        return Err(fail(format!(
            "icon \"{icon}\" isn't one the app draws; use one of {}",
            ICONS.join(", ")
        )));
    }
    let limit = match raw.limit {
        None => None,
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => Some(limit as usize),
        Some(limit) => {
            return Err(fail(format!(
                "limit {limit} is out of range; use 1 to {MAX_LIMIT}"
            )));
        }
    };
    Ok(Manifest {
        id: id.to_string(),
        title,
        kind,
        source,
        every,
        icon: raw.icon,
        wide: raw.wide.unwrap_or(false),
        limit,
        empty: raw
            .empty
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty()),
        path: None,
    })
}

/// toml's message with the line it's on, without the source excerpt.
fn toml_problem(error: &toml::de::Error, text: &str) -> String {
    let message = error.message().trim();
    match error.span() {
        Some(span) => {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!("line {line}: {message}")
        }
        None => message.to_string(),
    }
}

/// "30s", "1m", "5m", "1h" (a bare number is seconds); `None` when invalid.
pub fn parse_every(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (digits, unit) = match text.find(|c: char| !c.is_ascii_digit()) {
        Some(at) => text.split_at(at),
        None => (text, "s"),
    };
    if digits.is_empty() {
        return None;
    }
    let count: u64 = digits.parse().ok()?;
    let size = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    };
    Some(Duration::from_secs(count.checked_mul(size)?))
}

/// The first widgets, as `(file name, TOML)`, in Home's default order:
/// loose-ends, slack-waiting, today, gmail-needs-reply, memory-today,
/// working-now, devices, pull-requests.
pub fn builtins() -> &'static [(&'static str, &'static str)] {
    BUILTINS
}

const BUILTINS: &[(&str, &str)] = &[
    (
        "loose-ends.toml",
        r#"# Open loops the memory noticed: promises, half-done work, people waiting on you.
title  = "Loose ends"
kind   = "list"
source = "memory:loose-ends"
every  = "1m"
icon   = "flag"
empty  = "Nothing open"
"#,
    ),
    (
        "slack-waiting.toml",
        r#"# Slack threads and messages where someone is waiting on your answer.
title  = "Slack · waiting on you"
kind   = "list"
source = "keron-sources:slack-waiting"
every  = "1m"
icon   = "chat"
empty  = "Nobody's waiting"
"#,
    ),
    (
        "today.toml",
        r#"# Today's calendar events from every connected account.
title  = "Today"
kind   = "agenda"
source = "keron-sources:calendar-today"
every  = "1m"
icon   = "calendar"
empty  = "Nothing on the calendar"
"#,
    ),
    (
        "gmail-needs-reply.toml",
        r#"# Gmail threads that look like they need a reply from you.
title  = "Gmail · needs reply"
kind   = "list"
source = "keron-sources:gmail-needs-reply"
every  = "1m"
icon   = "mail"
empty  = "Inbox is answered"
"#,
    ),
    (
        "memory-today.toml",
        r#"# What the memory noted today, newest first.
title  = "Memory · today"
kind   = "timeline"
source = "memory:today"
every  = "1m"
icon   = "tree"
wide   = true
limit  = 8
empty  = "Nothing noted yet today"
"#,
    ),
    (
        "working-now.toml",
        r#"# Agent chats in this app that are working, waiting on you, or stuck on an error.
title  = "Working now"
kind   = "list"
source = "zeron:sessions"
every  = "5s"
icon   = "pulse"
empty  = "No agents running"
"#,
    ),
    (
        "devices.toml",
        r#"# Your devices, whether they're online, and what's running on them.
title  = "Devices"
kind   = "devices"
source = "zeron:devices"
every  = "30s"
icon   = "devices"
"#,
    ),
    (
        "pull-requests.toml",
        r#"# Pull requests your agent chats opened, open ones first.
title  = "Pull requests"
kind   = "list"
source = "zeron:pull-requests"
every  = "1m"
icon   = "pr"
empty  = "No open pull requests"
"#,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_in_homes_order() {
        let parsed: Vec<Manifest> = builtins()
            .iter()
            .map(|(file, text)| {
                assert!(text.starts_with("# "), "{file} starts with a comment");
                parse(file.strip_suffix(".toml").unwrap(), text).unwrap()
            })
            .collect();
        let summary: Vec<(&str, String, Duration)> = parsed
            .iter()
            .map(|m| (m.id.as_str(), m.source.to_string(), m.every))
            .collect();
        let s = Duration::from_secs;
        assert_eq!(
            summary,
            [
                ("loose-ends", "memory:loose-ends".to_string(), s(60)),
                (
                    "slack-waiting",
                    "keron-sources:slack-waiting".to_string(),
                    s(60)
                ),
                ("today", "keron-sources:calendar-today".to_string(), s(60)),
                (
                    "gmail-needs-reply",
                    "keron-sources:gmail-needs-reply".to_string(),
                    s(60)
                ),
                ("memory-today", "memory:today".to_string(), s(60)),
                ("working-now", "zeron:sessions".to_string(), s(5)),
                ("devices", "zeron:devices".to_string(), s(30)),
                ("pull-requests", "zeron:pull-requests".to_string(), s(60)),
            ]
        );
        let memory_today = &parsed[4];
        assert_eq!(
            (memory_today.kind, memory_today.wide, memory_today.limit),
            (Kind::Timeline, true, Some(8))
        );
        assert_eq!(parsed[2].kind, Kind::Agenda);
        assert_eq!(parsed[6].empty, None);
    }

    #[test]
    fn manifest_problems_name_the_file_and_the_mistake() {
        let base = "title = \"Weather\"\nkind = \"stat\"\nsource = \"script:weather.sh\"\n";
        let problem = |extra: &str| {
            parse("weather", &format!("{base}{extra}"))
                .unwrap_err()
                .to_string()
        };

        let typo = problem("colour = \"blue\"\n");
        assert!(typo.starts_with("weather.toml: line 4:"), "{typo}");
        assert!(typo.contains("colour"), "{typo}");
        assert!(problem("icon = \"rocket\"\n").contains("icon \"rocket\""));
        assert!(problem("every = \"2s\"\n").contains("too often"));
        assert!(problem("every = \"3000000h\"\n").contains("too long"));
        assert!(problem("limit = 0\n").contains("limit 0"));

        let zeron = parse(
            "w",
            "title = \"W\"\nkind = \"list\"\nsource = \"zeron:weather\"\n",
        )
        .unwrap_err();
        assert_eq!(zeron.file, "w.toml");
        assert!(zeron.message.contains("zeron:weather"), "{zeron}");
        assert!(parse("Bad Name", base).is_err());

        let ok = parse("weather", &format!("{base}every = 90\nicon = \"globe\"\n")).unwrap();
        assert_eq!(ok.every, Duration::from_secs(90));
        assert_eq!(ok.source, SourceSpec::Script("weather.sh".into()));
    }

    #[test]
    fn every_reads_seconds_minutes_and_hours() {
        assert_eq!(parse_every("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_every("45"), Some(Duration::from_secs(45)));
        assert_eq!(parse_every("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_every("1h"), Some(Duration::from_secs(3600)));
        for bad in ["", "m", "1d", "1.5m", "-5s", "1h30m"] {
            assert_eq!(parse_every(bad), None, "{bad}");
        }
    }
}
