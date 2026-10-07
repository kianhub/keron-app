//! keron-config: the owner's values for this build, from the app root's
//! `keron.toml` (see build.rs), plus Keron's fixed names. Everything that
//! points the app at the owner's infrastructure reads it from here.

use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/keron_config.rs"));

#[cfg(any(test, feature = "cli"))]
pub mod check;

/// Product name shown to people.
pub const APP_NAME: &str = "Keron";
/// The command-line binary.
pub const BINARY_NAME: &str = "keron";
/// URL scheme for OAuth callbacks (`keron://callback`) and app links.
pub const URL_SCHEME: &str = "keron";
/// systemd user unit for `keron daemon` on Linux.
pub const SYSTEMD_UNIT: &str = "keron.service";

/// True when keron.toml holds the owner's values rather than placeholders.
pub const fn is_configured() -> bool {
    PLACEHOLDERS.is_empty()
}

/// One line for logs and CLI errors when [`is_configured`] is false.
pub fn unconfigured_message() -> Option<String> {
    (!is_configured()).then(|| {
        format!(
            "this Keron build has placeholder settings ({}); fill in keron.toml and rebuild \
             to sign in and sync",
            PLACEHOLDERS.join(", ")
        )
    })
}

/// The app's data root: `~/.keron/app`. The rest of `~/.keron` (state/,
/// logs/, widgets/, home.toml) belongs to Keron's other parts.
pub fn data_dir_in(home: &Path) -> PathBuf {
    home.join(".keron").join("app")
}

/// Root of the self-updating managed install (`<root>/<version>` behind a
/// `current` symlink): `~/.keron/app/install`.
pub fn managed_install_root(home: &Path) -> PathBuf {
    data_dir_in(home).join("install")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_follow_the_prefix() {
        assert_eq!(MACOS_BUNDLE_ID, format!("{BUNDLE_PREFIX}.keron"));
        assert_eq!(IOS_BUNDLE_ID, format!("{BUNDLE_PREFIX}.keron.ios"));
        assert_eq!(MACOS_BUNDLE_ID_C.to_str().unwrap(), MACOS_BUNDLE_ID);
        assert_eq!(RELAY_URL, format!("https://{RELAY_HOST}"));
        assert!(LATEST_RELEASE_PAGE.starts_with(RELEASES_PAGE));
    }

    #[test]
    fn placeholders_never_resolve() {
        // A placeholder relay must sit under the reserved .invalid TLD, so a
        // debug build with placeholders can't reach anyone's server.
        if PLACEHOLDERS.contains(&"relay.host") {
            assert!(RELAY_HOST.ends_with(".invalid"), "{RELAY_HOST}");
        }
        assert_eq!(is_configured(), unconfigured_message().is_none());
    }

    #[test]
    fn nothing_points_at_zeron() {
        for value in [RELAY_URL, WORKOS_CLIENT_ID, RELEASES_PAGE, MACOS_BUNDLE_ID] {
            assert!(!value.contains("zeron"), "{value}");
        }
        assert!(RELEASES_URL.is_none_or(|url| url.starts_with(RELAY_URL)));
    }

    #[test]
    fn data_paths() {
        let home = Path::new("/Users/someone");
        assert_eq!(data_dir_in(home), Path::new("/Users/someone/.keron/app"));
        assert_eq!(
            managed_install_root(home),
            Path::new("/Users/someone/.keron/app/install")
        );
    }
}
