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

use std::collections::HashSet;
use std::io::{self, Write};
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
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(LayoutError::Read(e.to_string())),
        };
        toml::from_str(&text).map_err(|e| LayoutError::Parse(e.message().trim().to_string()))
    }

    /// Atomic write; creates the parent folder.
    pub fn save(&self, path: &Path) -> Result<(), LayoutError> {
        let text = toml::to_string_pretty(self).map_err(|e| LayoutError::Write(e.to_string()))?;
        write_atomic(path, text.as_bytes()).map_err(|e| LayoutError::Write(e.to_string()))
    }

    /// Every manifest in Home's order (shown or not): listed entries first,
    /// in layout order, then unlisted manifests in `manifests` order.
    pub fn arrange<'a>(&self, manifests: &'a [Manifest]) -> Vec<Placed<'a>> {
        let mut placed: Vec<Placed<'a>> = Vec::with_capacity(manifests.len());
        let mut seen = HashSet::new();
        for entry in &self.widgets {
            if !seen.insert(entry.id.as_str()) {
                continue;
            }
            if let Some(manifest) = manifests.iter().find(|m| m.id == entry.id) {
                placed.push(Placed {
                    manifest,
                    shown: entry.shown,
                    width: entry.width.map_or(default_width(manifest), clamp_width),
                });
            }
        }
        for manifest in manifests {
            if seen.insert(manifest.id.as_str()) {
                placed.push(Placed {
                    manifest,
                    shown: true,
                    width: default_width(manifest),
                });
            }
        }
        placed
    }

    /// Show or hide a widget.
    pub fn set_shown(&mut self, id: &str, shown: bool, manifests: &[Manifest]) {
        self.materialize(manifests);
        if let Some(entry) = self.widgets.iter_mut().find(|e| e.id == id) {
            entry.shown = shown;
        }
    }

    /// Move a widget to `index` in the full order (as [`Layout::arrange`]
    /// lists it); an index past the end moves it last.
    pub fn move_to(&mut self, id: &str, index: usize, manifests: &[Manifest]) {
        self.materialize(manifests);
        let present = |e: &LayoutEntry| manifests.iter().any(|m| m.id == e.id);
        // Entries whose manifest is gone keep their slots; the drawn ones
        // are reordered among the remaining slots.
        let slots: Vec<usize> = (0..self.widgets.len())
            .filter(|&i| present(&self.widgets[i]))
            .collect();
        let mut order: Vec<LayoutEntry> = slots.iter().map(|&i| self.widgets[i].clone()).collect();
        let Some(from) = order.iter().position(|e| e.id == id) else {
            return;
        };
        let entry = order.remove(from);
        order.insert(index.min(order.len()), entry);
        for (slot, entry) in slots.into_iter().zip(order) {
            self.widgets[slot] = entry;
        }
    }

    /// One or two columns (anything else is clamped).
    pub fn set_width(&mut self, id: &str, width: u8, manifests: &[Manifest]) {
        self.materialize(manifests);
        if let Some(entry) = self.widgets.iter_mut().find(|e| e.id == id) {
            entry.width = Some(clamp_width(width));
        }
    }

    /// List every widget explicitly: duplicates dropped (the first wins),
    /// then unlisted manifests appended in catalog order, so later edits
    /// apply to the order Home shows.
    fn materialize(&mut self, manifests: &[Manifest]) {
        let mut seen = HashSet::new();
        self.widgets.retain(|e| seen.insert(e.id.clone()));
        for manifest in manifests {
            if seen.insert(manifest.id.clone()) {
                self.widgets.push(LayoutEntry {
                    id: manifest.id.clone(),
                    shown: true,
                    width: None,
                });
            }
        }
    }
}

fn default_width(manifest: &Manifest) -> u8 {
    if manifest.wide { 2 } else { 1 }
}

fn clamp_width(width: u8) -> u8 {
    width.clamp(1, 2)
}

/// Write `bytes` to `path` atomically: a unique temp file next to it,
/// fsynced, renamed over the target, then the folder fsynced.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Each writer owns its temp file, so two windows saving at once can't
    // truncate or rename each other's half-written file.
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    fn widget(id: &str, wide: bool) -> Manifest {
        let text = format!(
            "title = \"{id}\"\nkind = \"list\"\nsource = \"script:{id}.sh\"\nwide = {wide}\n"
        );
        manifest::parse(id, &text).unwrap()
    }

    fn order(layout: &Layout, manifests: &[Manifest]) -> Vec<(String, bool, u8)> {
        layout
            .arrange(manifests)
            .iter()
            .map(|p| (p.manifest.id.clone(), p.shown, p.width))
            .collect()
    }

    fn row(id: &str, shown: bool, width: u8) -> (String, bool, u8) {
        (id.to_string(), shown, width)
    }

    #[test]
    fn arrange_and_edits_follow_the_full_order_and_survive_a_save() {
        let manifests = [
            widget("a", false),
            widget("b", true),
            widget("c", false),
            widget("new", false),
        ];
        let mut layout: Layout = toml::from_str(
            r#"
            [[widget]]
            id = "c"
            width = 7

            [[widget]]
            id = "gone"
            shown = false

            [[widget]]
            id = "a"
            shown = false
            "#,
        )
        .unwrap();
        // Listed first in layout order (gone skipped, width clamped), then
        // the unlisted ones in catalog order with the manifest's width.
        assert_eq!(
            order(&layout, &manifests),
            [
                row("c", true, 2),
                row("a", false, 1),
                row("b", true, 2),
                row("new", true, 1)
            ]
        );

        layout.move_to("new", 0, &manifests);
        layout.move_to("c", 99, &manifests);
        layout.set_shown("a", true, &manifests);
        layout.set_width("b", 0, &manifests);
        let expected = [
            row("new", true, 1),
            row("a", true, 1),
            row("b", true, 1),
            row("c", true, 2),
        ];
        assert_eq!(order(&layout, &manifests), expected);
        // The entry whose manifest is gone keeps its place.
        assert!(layout.widgets.iter().any(|e| e.id == "gone" && !e.shown));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/home.toml");
        layout.save(&path).unwrap();
        let loaded = Layout::load(&path).unwrap();
        assert_eq!(loaded, layout);
        assert_eq!(order(&loaded, &manifests), expected);
        // Only home.toml is left: the temp file was renamed over it.
        let files: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(files.len(), 1);

        // "gone" comes back where it was.
        let mut with_gone = manifests.to_vec();
        with_gone.push(widget("gone", false));
        let ids: Vec<String> = loaded
            .arrange(&with_gone)
            .iter()
            .map(|p| p.manifest.id.clone())
            .collect();
        assert_eq!(ids, ["new", "gone", "a", "b", "c"]);
    }

    #[test]
    fn missing_file_is_empty_and_a_broken_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Layout::load(&dir.path().join("home.toml")).unwrap(),
            Layout::default()
        );
        let broken = dir.path().join("broken.toml");
        std::fs::write(&broken, "[[widget]]\nshown = true\n").unwrap();
        assert!(matches!(Layout::load(&broken), Err(LayoutError::Parse(_))));
    }
}
