//! Application storage paths. Provider credentials keep their own locations.
//!
//! Keron keeps the app's data in `~/.keron/app` (the rest of `~/.keron`
//! belongs to Keron's other parts). Engine and harness code that finds its
//! folders through `ZERON_*` variables follows it via [`export_defaults`].

use std::ffi::OsString;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    resolve_data_dir(|name| std::env::var_os(name))
}

/// Set the `ZERON_*` folder variables the engine and harnesses read (data
/// dir, managed worktrees, Cursor state) to Keron's data root, unless the
/// environment already chose them. Children inherit them, so `keron mcp`
/// and agent processes agree with this one.
///
/// Must run before any other thread starts: setting the environment races
/// with concurrent reads.
pub fn export_defaults() {
    let data = data_dir();
    for (name, dir) in [
        ("ZERON_DATA_DIR", data.clone()),
        ("ZERON_WORKTREES_DIR", data.join("worktrees")),
        ("ZERON_CURSOR_STATE_DIR", data.join("cursor-state")),
    ] {
        if std::env::var_os(name).is_none_or(|value| value.is_empty()) {
            // SAFETY: called first thing in `main`, before any thread exists.
            unsafe { std::env::set_var(name, dir) };
        }
    }
}

fn resolve_data_dir(mut env: impl FnMut(&str) -> Option<OsString>) -> PathBuf {
    if let Some(dir) = env("ZERON_DATA_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        // Explorer does not set HOME. Do not let a shell-specific HOME select
        // a different workspace from a desktop launch, or migrate credentials
        // between Unix-style and native Windows directories implicitly.
        let local = env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            })
            .expect("LOCALAPPDATA and USERPROFILE not set; set ZERON_DATA_DIR");
        local.join("Keron")
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(env("HOME").expect("HOME not set"));
        keron_config::data_dir_in(&home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(vars: &[(&str, &str)]) -> PathBuf {
        resolve_data_dir(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.into())
        })
    }

    #[test]
    fn explicit_data_dir_needs_no_home() {
        assert_eq!(
            resolve(&[("ZERON_DATA_DIR", "custom data")]),
            PathBuf::from("custom data")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn default_is_keron_app_folder() {
        assert_eq!(
            resolve(&[("HOME", "/Users/someone")]),
            PathBuf::from("/Users/someone/.keron/app"),
        );
        assert_eq!(
            resolve(&[("HOME", "/Users/someone"), ("ZERON_DATA_DIR", "")]),
            PathBuf::from("/Users/someone/.keron/app"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn explorer_launch_without_home_uses_local_app_data() {
        assert_eq!(
            resolve(&[("LOCALAPPDATA", r"C:\Users\Test User\AppData\Local")]),
            PathBuf::from(r"C:\Users\Test User\AppData\Local\Keron"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_fallback_handles_unicode_and_apostrophes() {
        assert_eq!(
            resolve(&[("USERPROFILE", r"C:\Users\O'Brien 日本語")]),
            PathBuf::from(r"C:\Users\O'Brien 日本語\AppData\Local\Keron"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_does_not_depend_on_shell_home() {
        assert_eq!(
            resolve(&[("HOME", r"D:\msys-home"), ("LOCALAPPDATA", r"C:\Local")]),
            PathBuf::from(r"C:\Local\Keron"),
        );
    }
}
