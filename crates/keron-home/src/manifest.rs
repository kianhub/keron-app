//! One widget's manifest: `~/.keron/widgets/<id>.toml`.
//!
//! ```toml
//! title  = "Slack · waiting on you"
//! kind   = "list"            # list, timeline, stat, agenda, devices
//! source = "keron-sources:slack-waiting"
//! every  = "1m"              # optional, default 1m, at least 5s
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

use crate::{Kind, SourceSpec};

/// Icon names a manifest may use; the app draws each one.
pub const ICONS: &[&str] = &[
    "flag", "chat", "check", "calendar", "tree", "pulse", "devices", "pr", "mail", "mic", "bars",
    "widget", "bell", "star", "list", "globe",
];

/// Default refresh.
pub const DEFAULT_EVERY: Duration = Duration::from_secs(60);
/// Fastest refresh a manifest may ask for.
pub const MIN_EVERY: Duration = Duration::from_secs(5);

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
    let _ = id;
    todo!("keron-home: valid_id")
}

/// Parse a manifest's TOML. `id` is the file stem.
pub fn parse(id: &str, text: &str) -> Result<Manifest, ManifestError> {
    let _ = (id, text);
    todo!("keron-home: manifest::parse")
}

/// "30s", "1m", "5m", "1h" (a bare number is seconds); `None` when invalid.
pub fn parse_every(text: &str) -> Option<Duration> {
    let _ = text;
    todo!("keron-home: parse_every")
}

/// The first widgets, as `(file name, TOML)`, in Home's default order:
/// loose-ends, slack-waiting, today, gmail-needs-reply, memory-today,
/// working-now, devices, pull-requests.
pub fn builtins() -> &'static [(&'static str, &'static str)] {
    todo!("keron-home: manifest::builtins")
}
