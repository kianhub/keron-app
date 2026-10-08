//! The widgets folder: loading every manifest, seeding the first widgets,
//! and watching for changes so a new widget appears without a rebuild.

use std::time::Duration;

use crate::{HomePaths, Manifest, ManifestError};

/// Every widget found, plus the files that didn't parse (shown in
/// Customize so a broken manifest isn't silently missing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    /// Sorted by id.
    pub manifests: Vec<Manifest>,
    pub problems: Vec<ManifestError>,
}

/// Read every `*.toml` in the widgets folder (not recursive; hidden files
/// and other extensions are skipped). A missing folder gives the built-ins.
pub fn load(paths: &HomePaths) -> Catalog {
    let _ = paths;
    todo!("keron-home: catalog::load")
}

/// When the widgets folder doesn't exist yet: create it and write the
/// built-in manifests and README.md (WIDGETS.md). Returns whether it
/// seeded. An existing folder is never touched, so a widget the owner
/// deleted stays deleted.
pub fn seed(paths: &HomePaths) -> std::io::Result<bool> {
    let _ = paths;
    todo!("keron-home: catalog::seed")
}

/// How long changes settle before [`Watcher::changed`] wakes.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// Watches the widgets folder (non-recursive). Dropping it stops watching.
pub struct Watcher {
    // The implementation keeps its notify watcher and channel here.
    _private: (),
}

impl Watcher {
    /// Start watching. Must be called inside a tokio runtime (FSEvents
    /// registration blocks, so it runs on a blocking thread).
    pub async fn start(paths: &HomePaths) -> Result<Self, String> {
        let _ = paths;
        todo!("keron-home: Watcher::start")
    }

    /// Wait for the next change, debounced; `None` once the watcher stops.
    pub async fn changed(&mut self) -> Option<()> {
        todo!("keron-home: Watcher::changed")
    }
}
