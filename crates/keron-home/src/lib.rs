//! keron-home: Home's widget runtime (docs/plan/04-home.md in the Keron repo).
//!
//! A widget is a manifest plus a source. The app draws a handful of kinds
//! (list, timeline, stat, agenda, devices); a new kind needs Rust, a new
//! widget doesn't: drop `<id>.toml` (and, for a `script:` source, the script)
//! into `~/.keron/widgets` and it appears. The format is in `WIDGETS.md`
//! next to this crate's Cargo.toml, which is also seeded into the widgets
//! folder as README.md.
//!
//! - [`manifest`]: one widget's TOML.
//! - [`source`]: where a widget's data comes from.
//! - [`kinds`]: the JSON each kind takes, parsed leniently.
//! - [`heat`]: loose-ends heat levels and words.
//! - [`layout`]: `~/.keron/home.toml`, which widgets show, in what order, how wide.
//! - [`catalog`]: loading, seeding and watching the widgets folder.
//! - [`script`]: running a `script:` source.
//! - [`zeron`]: the app's own data as a plain snapshot, turned into payloads.
//! - [`fetch`]: fetching a widget's payload.
//! - [`loose_ends`]: row actions through the door: snooze, done, dismiss and
//!   shown on loose ends, done on Gmail and Slack rows.

use std::path::{Path, PathBuf};

pub mod catalog;
pub mod fetch;
pub mod heat;
pub mod kinds;
pub mod layout;
pub mod loose_ends;
pub mod manifest;
pub mod script;
pub mod source;
pub mod zeron;

pub use fetch::{FetchError, Fetcher};
pub use heat::Heat;
pub use kinds::{
    AgendaItem, Body, DeviceItem, Kind, ListItem, Meter, Payload, PayloadError, Stat, TimelineItem,
};
pub use layout::{Layout, LayoutEntry, Placed};
pub use manifest::{Manifest, ManifestError};
pub use source::SourceSpec;
pub use zeron::ZeronSnapshot;

/// Where Home's files live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomePaths {
    /// `~/.keron/widgets`: manifests, scripts, README.md.
    pub widgets_dir: PathBuf,
    /// `~/.keron/home.toml`.
    pub layout_file: PathBuf,
}

impl HomePaths {
    /// The standard places under a home folder.
    pub fn under_home(home: &Path) -> Self {
        Self {
            widgets_dir: keron_config::widgets_dir_in(home),
            layout_file: keron_config::home_layout_in(home),
        }
    }
}
