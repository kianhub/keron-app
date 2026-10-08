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

/// The engine's loopback IPC port (`ZERON_IPC_PORT` overrides it). Not
/// Zeron's 27654, so Keron and a stock Zeron on the same Mac never dial
/// each other's engine.
pub const IPC_PORT: u16 = 27754;
/// The desktop sign-in's loopback callback port (`ZERON_CALLBACK_PORT`
/// overrides it). The relay's WorkOS app lists
/// `http://127.0.0.1:27741/callback` as a redirect URI (edge/RELAY.md).
pub const SIGN_IN_CALLBACK_PORT: u16 = 27741;

/// Loopback port of the app's own sign-in to the public door (Home). Its
/// dynamically registered client lists `http://127.0.0.1:27742/callback`;
/// if the port is taken the app registers again on a free one.
pub const DOOR_SIGN_IN_CALLBACK_PORT: u16 = 27742;

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

/// Keron's root on this machine: `~/.keron`.
pub fn keron_root_in(home: &Path) -> PathBuf {
    home.join(".keron")
}

/// Home's widget manifests and scripts: `~/.keron/widgets`.
pub fn widgets_dir_in(home: &Path) -> PathBuf {
    keron_root_in(home).join("widgets")
}

/// Home's layout (which widgets show, their order and width): `~/.keron/home.toml`.
pub fn home_layout_in(home: &Path) -> PathBuf {
    keron_root_in(home).join("home.toml")
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
        for value in [RELAY_URL, DOOR_URL, WORKOS_CLIENT_ID, RELEASES_PAGE, MACOS_BUNDLE_ID] {
            assert!(!value.contains("zeron"), "{value}");
        }
        assert!(RELEASES_URL.is_none_or(|url| url.starts_with(RELAY_URL)));
        assert_eq!(DOOR_URL, format!("https://{DOOR_HOST}"));
        assert_eq!(DOOR_RESOURCE, format!("{DOOR_URL}/mcp"));
        assert_ne!(DOOR_HOST, RELAY_HOST, "the door and the relay are different services");
    }

    #[test]
    fn ports_are_not_zerons() {
        // Zeron's defaults: IPC 27654, sign-in callback 27641.
        assert!(![27654, 27641].contains(&IPC_PORT));
        assert!(![27654, 27641].contains(&SIGN_IN_CALLBACK_PORT));
        assert_ne!(IPC_PORT, SIGN_IN_CALLBACK_PORT);
        assert!(![27654, 27641, IPC_PORT, SIGN_IN_CALLBACK_PORT].contains(&DOOR_SIGN_IN_CALLBACK_PORT));
    }

    #[test]
    fn data_paths() {
        let home = Path::new("/Users/someone");
        assert_eq!(data_dir_in(home), Path::new("/Users/someone/.keron/app"));
        assert_eq!(
            managed_install_root(home),
            Path::new("/Users/someone/.keron/app/install")
        );
        assert_eq!(widgets_dir_in(home), Path::new("/Users/someone/.keron/widgets"));
        assert_eq!(home_layout_in(home), Path::new("/Users/someone/.keron/home.toml"));
    }
}
