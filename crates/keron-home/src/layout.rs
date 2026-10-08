//! `~/.keron/home.toml`: which widgets show, in what order, how wide.
//!
//! ```toml
//! [[widget]]
//! id = "loose-ends"
//! shown = true
//! width = 2      # 1 or 2 columns; missing means the manifest's `wide`
//! ```
//!
//! The order of the `[[widget]]` tables is the order on Home. A manifest the
//! layout doesn't mention yet (a widget just added) shows, at the end. An
//! entry whose manifest is gone is kept, so a widget that comes back keeps
//! its place, and isn't drawn. Saving is atomic (temp file, fsync, rename).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Manifest;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default, rename = "widget")]
    pub widgets: Vec<LayoutEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutEntry {
    pub id: String,
    #[serde(default = "shown_by_default")]
    pub shown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u8>,
}

fn shown_by_default() -> bool {
    true
}

/// A widget as Home draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed<'a> {
    pub manifest: &'a Manifest,
    pub shown: bool,
    /// 1 or 2 columns.
    pub width: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("can't read home.toml: {0}")]
    Read(String),
    #[error("home.toml isn't valid: {0}")]
    Parse(String),
    #[error("can't save home.toml: {0}")]
    Write(String),
}

impl Layout {
    /// A missing file is an empty layout (every widget shows, in the
    /// catalog's order).
    pub fn load(path: &Path) -> Result<Self, LayoutError> {
        let _ = path;
        todo!("keron-home: Layout::load")
    }

    /// Atomic write; creates the parent folder.
    pub fn save(&self, path: &Path) -> Result<(), LayoutError> {
        let _ = path;
        todo!("keron-home: Layout::save")
    }

    /// Every manifest in Home's order (shown or not): listed entries first,
    /// in layout order, then unlisted manifests in `manifests` order.
    pub fn arrange<'a>(&self, manifests: &'a [Manifest]) -> Vec<Placed<'a>> {
        let _ = manifests;
        todo!("keron-home: Layout::arrange")
    }

    /// Show or hide a widget.
    pub fn set_shown(&mut self, id: &str, shown: bool, manifests: &[Manifest]) {
        let _ = (id, shown, manifests);
        todo!("keron-home: Layout::set_shown")
    }

    /// Move a widget to `index` in the full order (as [`Layout::arrange`]
    /// lists it); an index past the end moves it last.
    pub fn move_to(&mut self, id: &str, index: usize, manifests: &[Manifest]) {
        let _ = (id, index, manifests);
        todo!("keron-home: Layout::move_to")
    }

    /// One or two columns (anything else is clamped).
    pub fn set_width(&mut self, id: &str, width: u8, manifests: &[Manifest]) {
        let _ = (id, width, manifests);
        todo!("keron-home: Layout::set_width")
    }
}
