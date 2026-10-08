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
        let text = text.trim();
        let Some((prefix, rest)) = text.split_once(':') else {
            return Err(format!(
                "source \"{text}\" should start with keron-sources:, memory:, zeron: or script:"
            ));
        };
        if prefix == "script" {
            if rest.trim().is_empty() {
                return Err("script: needs a path, like script:my-widget.sh".to_string());
            }
            return Ok(SourceSpec::Script(PathBuf::from(rest.trim())));
        }
        let make: fn(String) -> SourceSpec = match prefix {
            "keron-sources" => SourceSpec::KeronSources,
            "memory" => SourceSpec::Memory,
            "zeron" => SourceSpec::Zeron,
            _ => {
                return Err(format!(
                    "source \"{text}\" should start with keron-sources:, memory:, zeron: or script:"
                ));
            }
        };
        if !valid_name(rest) {
            return Err(format!(
                "\"{rest}\" isn't a valid {prefix} name (lowercase letters, digits and -)"
            ));
        }
        Ok(make(rest.to_string()))
    }

    /// The door path for door sources: `/sources/<name>` or `/memory/<name>`.
    pub fn door_path(&self) -> Option<String> {
        match self {
            SourceSpec::KeronSources(name) => Some(format!("/sources/{name}")),
            SourceSpec::Memory(name) => Some(format!("/memory/{name}")),
            SourceSpec::Zeron(_) | SourceSpec::Script(_) => None,
        }
    }
}

/// `[a-z0-9][a-z0-9-]*`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl fmt::Display for SourceSpec {
    /// Back to the manifest form, `keron-sources:slack-waiting` and so on.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceSpec::KeronSources(name) => write!(f, "keron-sources:{name}"),
            SourceSpec::Memory(name) => write!(f, "memory:{name}"),
            SourceSpec::Zeron(name) => write!(f, "zeron:{name}"),
            SourceSpec::Script(path) => write!(f, "script:{}", path.display()),
        }
    }
}
