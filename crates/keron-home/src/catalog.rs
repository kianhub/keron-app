//! The widgets folder: loading every manifest, seeding the first widgets,
//! and watching for changes so a new widget appears without a rebuild.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::{HomePaths, Manifest, ManifestError, manifest};

/// The owner-facing format reference, seeded as README.md.
pub const README: &str = include_str!("../WIDGETS.md");

/// Manifests bigger than this are refused rather than read.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// Every widget found, plus the files that didn't parse (shown in
/// Customize so a broken manifest isn't silently missing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    /// The built-ins first, in Home's default order, then the rest by id,
    /// so an empty layout shows them the way [`manifest::builtins`] lists.
    pub manifests: Vec<Manifest>,
    pub problems: Vec<ManifestError>,
}

/// Read every `*.toml` in the widgets folder (not recursive; hidden files
/// and other extensions are skipped). A missing folder gives the built-ins.
pub fn load(paths: &HomePaths) -> Catalog {
    let entries = match std::fs::read_dir(&paths.widgets_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return builtin_catalog(),
        Err(e) => {
            return Catalog {
                manifests: Vec::new(),
                problems: vec![ManifestError {
                    file: "widgets".to_string(),
                    message: format!("can't read the widgets folder: {e}"),
                }],
            };
        }
    };
    let mut catalog = Catalog::default();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        if name.starts_with('.') || !path.is_file() {
            continue;
        }
        let problem = |message: String| ManifestError {
            file: name.to_string(),
            message,
        };
        match read_manifest(&path) {
            Ok(text) => match manifest::parse(id, &text) {
                Ok(mut manifest) => {
                    manifest.path = Some(path.clone());
                    catalog.manifests.push(manifest);
                }
                Err(e) => catalog.problems.push(e),
            },
            Err(message) => catalog.problems.push(problem(message)),
        }
    }
    sort(&mut catalog.manifests);
    catalog.problems.sort_by(|a, b| a.file.cmp(&b.file));
    catalog
}

fn read_manifest(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("can't read it: {e}"))?
        .len();
    if size > MAX_MANIFEST_BYTES {
        return Err(format!(
            "it's bigger than {} KiB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("can't read it: {e}"))
}

fn sort(manifests: &mut [Manifest]) {
    let builtin_rank = |id: &str| {
        manifest::builtins()
            .iter()
            .position(|(file, _)| file.strip_suffix(".toml") == Some(id))
            .unwrap_or(usize::MAX)
    };
    manifests.sort_by(|a, b| (builtin_rank(&a.id), &a.id).cmp(&(builtin_rank(&b.id), &b.id)));
}

fn builtin_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    for (file, text) in manifest::builtins() {
        let id = file.strip_suffix(".toml").unwrap_or(file);
        match manifest::parse(id, text) {
            Ok(manifest) => catalog.manifests.push(manifest),
            Err(e) => catalog.problems.push(e),
        }
    }
    sort(&mut catalog.manifests);
    catalog
}

/// When the widgets folder doesn't exist yet: create it and write the
/// built-in manifests and README.md (WIDGETS.md). Returns whether it
/// seeded. An existing folder is never touched, so a widget the owner
/// deleted stays deleted.
pub fn seed(paths: &HomePaths) -> std::io::Result<bool> {
    let dir = &paths.widgets_dir;
    if std::fs::symlink_metadata(dir).is_ok() {
        return Ok(false);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        // Another window seeded it first.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    }
    let files = manifest::builtins()
        .iter()
        .copied()
        .chain([("README.md", README)]);
    for (name, text) in files {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))?;
        file.write_all(text.as_bytes())?;
    }
    Ok(true)
}

/// How long changes settle before [`Watcher::changed`] wakes.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// Watches the widgets folder (non-recursive). Dropping it stops watching.
pub struct Watcher {
    _watcher: notify::RecommendedWatcher,
    events: mpsc::UnboundedReceiver<()>,
}

impl Watcher {
    /// Start watching. Must be called inside a tokio runtime (FSEvents
    /// registration blocks, so it runs on a blocking thread).
    pub async fn start(paths: &HomePaths) -> Result<Self, String> {
        let dir = paths.widgets_dir.clone();
        let (tx, events) = mpsc::unbounded_channel();
        let watcher = tokio::task::spawn_blocking(move || {
            use notify::Watcher as _;
            let mut watcher = notify::recommended_watcher(
                move |event: notify::Result<notify::Event>| match event {
                    Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                        let _ = tx.send(());
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!(error = %e, "home: widgets watch error"),
                },
            )
            .map_err(|e| format!("can't watch the widgets folder: {e}"))?;
            watcher
                .watch(&dir, notify::RecursiveMode::NonRecursive)
                .map_err(|e| format!("can't watch the widgets folder: {e}"))?;
            Ok::<_, String>(watcher)
        })
        .await
        .map_err(|e| format!("can't watch the widgets folder: {e}"))??;
        Ok(Self {
            _watcher: watcher,
            events,
        })
    }

    /// Wait for the next change, debounced; `None` once the watcher stops.
    pub async fn changed(&mut self) -> Option<()> {
        self.events.recv().await?;
        loop {
            match tokio::time::timeout(DEBOUNCE, self.events.recv()).await {
                Ok(Some(())) => continue,
                Ok(None) | Err(_) => return Some(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> HomePaths {
        HomePaths::under_home(root)
    }

    fn ids(catalog: &Catalog) -> Vec<&str> {
        catalog.manifests.iter().map(|m| m.id.as_str()).collect()
    }

    const BUILTIN_IDS: [&str; 8] = [
        "loose-ends",
        "slack-waiting",
        "today",
        "gmail-needs-reply",
        "memory-today",
        "working-now",
        "devices",
        "pull-requests",
    ];

    #[test]
    fn seeds_once_and_loads_owner_widgets_after_the_builtins() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());

        let missing = load(&paths);
        assert_eq!(ids(&missing), BUILTIN_IDS);
        assert!(missing.manifests.iter().all(|m| m.path.is_none()));

        assert!(seed(&paths).unwrap());
        let readme = std::fs::read_to_string(paths.widgets_dir.join("README.md")).unwrap();
        assert_eq!(readme, README);

        // The owner deletes a built-in, adds two widgets, one broken, and
        // some files that aren't manifests.
        let dir = &paths.widgets_dir;
        std::fs::remove_file(dir.join("devices.toml")).unwrap();
        std::fs::write(
            dir.join("aaa.toml"),
            "title = \"A\"\nkind = \"stat\"\nsource = \"script:aaa.sh\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("broken.toml"), "title = \"B\"\n").unwrap();
        std::fs::write(dir.join(".hidden.toml"), "nonsense").unwrap();
        std::fs::write(dir.join("notes.txt"), "nonsense").unwrap();
        std::fs::create_dir(dir.join("sub.toml")).unwrap();

        assert!(!seed(&paths).unwrap());
        assert!(
            !dir.join("devices.toml").exists(),
            "seed left the folder alone"
        );

        let catalog = load(&paths);
        let mut expected: Vec<&str> = BUILTIN_IDS
            .into_iter()
            .filter(|id| *id != "devices")
            .collect();
        expected.push("aaa");
        assert_eq!(ids(&catalog), expected);
        assert_eq!(
            catalog.manifests.last().unwrap().path,
            Some(dir.join("aaa.toml"))
        );
        assert_eq!(catalog.problems.len(), 1);
        assert_eq!(catalog.problems[0].file, "broken.toml");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_sees_a_new_widget() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        std::fs::create_dir_all(&paths.widgets_dir).unwrap();
        let mut watcher = Watcher::start(&paths).await.unwrap();
        // Drop anything left over from creating the folder, so only the new
        // file can wake it.
        tokio::time::sleep(Duration::from_millis(500)).await;
        while watcher.events.try_recv().is_ok() {}

        let file = paths.widgets_dir.join("new.toml");
        std::fs::write(
            &file,
            "title = \"New\"\nkind = \"list\"\nsource = \"zeron:sessions\"\n",
        )
        .unwrap();
        let woke = tokio::time::timeout(Duration::from_secs(10), watcher.changed()).await;
        assert_eq!(woke, Ok(Some(())));
        assert_eq!(ids(&load(&paths)), ["new"]);
    }
}
