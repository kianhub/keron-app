//! Where a widget's data comes from: the `source = "..."` line.

use std::fmt;
use std::path::PathBuf;

/// `keron-sources:<name>`, `memory:<name>`, `zeron:<name>` or `script:<path>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceSpec {
    /// JSON from the mini's keron-sources, `GET /sources/<name>` on the door.
    KeronSources(String),
    /// JSON from keron-memory, `GET /memory/<name>` on the door
    /// (`loose-ends`, `today`).
    Memory(String),
    /// Data the app already has (`sessions`, `devices`, `pull-requests`,
    /// `services`, `todos`), answered from [`crate::ZeronSnapshot`].
    Zeron(String),
    /// A local program that prints the payload as JSON. The path is as
    /// written: relative paths are under the widgets folder, `~/` is the
    /// home folder.
    Script(PathBuf),
}

impl SourceSpec {
    /// Parse a manifest's `source` value. Names are `[a-z0-9][a-z0-9-]*`;
    /// a script path is any non-empty path.
    pub fn parse(text: &str) -> Result<Self, String> {
        let _ = text;
        todo!("keron-home: SourceSpec::parse")
    }

    /// The door path for door sources: `/sources/<name>` or `/memory/<name>`.
    pub fn door_path(&self) -> Option<String> {
        todo!("keron-home: SourceSpec::door_path")
    }
}

impl fmt::Display for SourceSpec {
    /// Back to the manifest form, `keron-sources:slack-waiting` and so on.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!("keron-home: SourceSpec Display")
    }
}
