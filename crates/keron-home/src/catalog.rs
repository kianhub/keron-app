//! The widgets folder: loading every manifest, seeding the first widgets,
//! and watching for changes so a new widget appears without a rebuild.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::{HomePaths, Manifest, ManifestError, manifest};

/// The owner-facing format reference, seeded as README.md.
pub const README: &str = include_str!("../WIDGETS.md");

/// [`fingerprint`]s of every README.md the app has written, oldest first;
/// the last is [`README`]'s. A README.md matching one is an unedited copy,
/// so [`refresh_readme`] may replace it. When WIDGETS.md changes, append its
/// new fingerprint (a test says so).
const SEEDED_READMES: [u64; 7] = [
    0x44002d8dae033d70,
    0x97faf17121760427,
    0x08fd2bb9e7d63fa6,
    0x93a22c72952120cf,
    0xd7812d38f2dca372,
    0x7c514e1be6f33dcc,
    0xc8d6b755772d971a,
];

/// The hidden file listing the built-ins this folder has had (seeded or
/// added), one id per line, so Customize offers each new one only once.
const HAD_FILE: &str = ".builtins";

/// The built-ins a folder seeded before [`HAD_FILE`] existed got.
const FIRST_BUILTINS: [&str; 8] = [
    "loose-ends",
    "slack-waiting",
    "today",
    "gmail-needs-reply",
    "memory-today",
    "working-now",
    "devices",
    "pull-requests",
];

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
    /// The built-in ids this folder has had (see [`HAD_FILE`]); Customize
    /// doesn't offer them again.
    pub had_builtins: Vec<String>,
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
                had_builtins: Vec::new(),
            };
        }
    };
    let mut catalog = Catalog {
        had_builtins: had_builtins(paths),
        ..Catalog::default()
    };
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
    let had: String = manifest::builtins()
        .iter()
        .filter_map(|(file, _)| file.strip_suffix(".toml"))
        .map(|id| format!("{id}\n"))
        .collect();
    let files = manifest::builtins()
        .iter()
        .copied()
        .chain([("README.md", README), (HAD_FILE, had.as_str())]);
    for (name, text) in files {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))?;
        file.write_all(text.as_bytes())?;
    }
    Ok(true)
}

/// Built-ins the app gained after the widgets folder was seeded (seeding
/// happens once), as `(id, title)` in Home's order. Customize offers them;
/// nothing adds them by itself. One the folder has had isn't offered (the
/// owner deleted it), nor one with a file there, even a broken one, nor one
/// whose source another widget already shows. Nothing is offered when the
/// folder couldn't be read.
pub fn missing_builtins(catalog: &Catalog) -> Vec<(&'static str, String)> {
    if catalog.problems.iter().any(|p| p.file == "widgets") {
        return Vec::new();
    }
    manifest::builtins()
        .iter()
        .filter_map(|(file, text)| {
            let id = file.strip_suffix(".toml")?;
            let present = catalog.had_builtins.iter().any(|had| had == id)
                || catalog.manifests.iter().any(|m| m.id == id)
                || catalog.problems.iter().any(|p| p.file == *file);
            if present {
                return None;
            }
            let builtin = manifest::parse(id, text).ok()?;
            let shown_elsewhere = catalog.manifests.iter().any(|m| m.source == builtin.source);
            (!shown_elsewhere).then_some((id, builtin.title))
        })
        .collect()
}

/// The built-ins the folder has had: [`HAD_FILE`]'s ids, or
/// [`FIRST_BUILTINS`] for a folder seeded before it existed. When the file
/// can't be read, every built-in, so nothing is offered.
fn had_builtins(paths: &HomePaths) -> Vec<String> {
    match std::fs::read_to_string(paths.widgets_dir.join(HAD_FILE)) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            FIRST_BUILTINS.iter().map(|id| id.to_string()).collect()
        }
        Err(_) => manifest::builtins()
            .iter()
            .filter_map(|(file, _)| file.strip_suffix(".toml"))
            .map(str::to_string)
            .collect(),
    }
}

/// Write the built-in `id`'s manifest into the widgets folder, creating the
/// folder if it's missing. Never overwrites: `Ok(false)` when the file is
/// already there. An id that isn't a built-in is an error.
pub fn add_builtin(paths: &HomePaths, id: &str) -> io::Result<bool> {
    let file = format!("{id}.toml");
    let Some((_, text)) = manifest::builtins().iter().find(|(name, _)| *name == file) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{id} isn't a built-in widget"),
        ));
    };
    std::fs::create_dir_all(&paths.widgets_dir)?;
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(paths.widgets_dir.join(&file));
    let added = match created {
        Ok(mut out) => {
            out.write_all(text.as_bytes())?;
            true
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e),
    };
    let mut had = had_builtins(paths);
    if !had.iter().any(|known| known == id) {
        had.push(id.to_string());
        let text: String = had.iter().map(|id| format!("{id}\n")).collect();
        std::fs::write(paths.widgets_dir.join(HAD_FILE), text)?;
    }
    Ok(added)
}

/// Replace README.md with [`README`] when it's an unedited copy an older app
/// wrote (its fingerprint is in [`SEEDED_READMES`]), so the folder's format
/// reference names what this app can do. An edited or missing README is
/// left alone. Returns whether it rewrote.
pub fn refresh_readme(paths: &HomePaths) -> io::Result<bool> {
    refresh_readme_from(paths, &SEEDED_READMES)
}

fn refresh_readme_from(paths: &HomePaths, seeded: &[u64]) -> io::Result<bool> {
    let path = paths.widgets_dir.join("README.md");
    let text = match std::fs::read(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let print = fingerprint(&text);
    if text == README.as_bytes() || !seeded.contains(&print) {
        return Ok(false);
    }
    let tmp = paths.widgets_dir.join(".README.md.tmp");
    std::fs::write(&tmp, README)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

/// FNV-1a over the bytes: stable across builds, unlike std's hasher.
fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
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

    const BUILTIN_IDS: [&str; 9] = [
        "loose-ends",
        "slack-waiting",
        "today",
        "gmail-needs-reply",
        "memory-today",
        "working-now",
        "devices",
        "pull-requests",
        "usage",
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

    #[test]
    fn a_builtin_the_folder_never_had_is_offered_until_added() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        assert!(seed(&paths).unwrap());
        assert!(missing_builtins(&load(&paths)).is_empty());
        let file = paths.widgets_dir.join("usage.toml");
        let had = paths.widgets_dir.join(HAD_FILE);

        // A folder seeded before Usage existed: no usage.toml, no record.
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_file(&had).unwrap();
        assert_eq!(
            missing_builtins(&load(&paths)),
            [("usage", "Usage".to_string())]
        );

        assert!(add_builtin(&paths, "usage").unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            manifest::builtins().last().unwrap().1
        );
        assert!(missing_builtins(&load(&paths)).is_empty());
        // The owner edits it; adding again never overwrites.
        std::fs::write(
            &file,
            "title = \"Mine\"\nkind = \"list\"\nsource = \"zeron:usage\"\n",
        )
        .unwrap();
        assert!(!add_builtin(&paths, "usage").unwrap());
        assert!(std::fs::read_to_string(&file).unwrap().contains("Mine"));
        assert!(add_builtin(&paths, "weather").is_err());

        // Deleted after adding (or a deleted original): not offered again.
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_file(paths.widgets_dir.join("devices.toml")).unwrap();
        assert!(missing_builtins(&load(&paths)).is_empty());
    }

    #[test]
    fn a_builtin_is_not_offered_when_another_widget_shows_its_source() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        assert!(seed(&paths).unwrap());
        std::fs::remove_file(paths.widgets_dir.join("usage.toml")).unwrap();
        std::fs::remove_file(paths.widgets_dir.join(HAD_FILE)).unwrap();
        // The chat wrote the owner's own usage widget.
        std::fs::write(
            paths.widgets_dir.join("my-usage.toml"),
            "title = \"Plans\"\nkind = \"list\"\nsource = \"zeron:usage\"\n",
        )
        .unwrap();
        assert!(missing_builtins(&load(&paths)).is_empty());
    }

    #[test]
    fn readme_is_refreshed_only_when_the_owner_left_it_as_seeded() {
        assert_eq!(
            SEEDED_READMES.last(),
            Some(&fingerprint(README.as_bytes())),
            "WIDGETS.md changed: append its fingerprint to SEEDED_READMES"
        );
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        assert!(seed(&paths).unwrap());
        let readme = paths.widgets_dir.join("README.md");
        assert!(!refresh_readme(&paths).unwrap());

        // An owner-edited README stays.
        std::fs::write(&readme, "my notes").unwrap();
        assert!(!refresh_readme(&paths).unwrap());
        assert_eq!(std::fs::read_to_string(&readme).unwrap(), "my notes");

        // A copy an older app wrote is replaced (its text isn't kept here,
        // only its fingerprint, so stand one in).
        std::fs::write(&readme, "an old README").unwrap();
        let seeded = [fingerprint(b"an old README"), SEEDED_READMES[4]];
        assert!(refresh_readme_from(&paths, &seeded).unwrap());
        assert_eq!(std::fs::read_to_string(&readme).unwrap(), README);

        std::fs::remove_file(&readme).unwrap();
        assert!(
            !refresh_readme(&paths).unwrap(),
            "a deleted README stays deleted"
        );
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
