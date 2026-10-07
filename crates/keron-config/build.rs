//! Reads the app root's `keron.toml`, validates it, and writes the owner
//! values as constants into `$OUT_DIR/keron_config.rs` (included by lib.rs).
//!
//! A malformed value fails every build. A placeholder (any value containing
//! "change-me") fails release-profile builds, so a packaged app or an iPhone
//! archive can never ship pointing at nothing; `KERON_ALLOW_PLACEHOLDERS=1`
//! lifts that for CI test runs.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    relay: Relay,
    apple: Apple,
    updates: Updates,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Relay {
    host: String,
    workos_client_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apple {
    team_id: String,
    bundle_prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Updates {
    enabled: bool,
    releases_page: String,
}

const PLACEHOLDER_MARK: &str = "change-me";

fn is_placeholder(value: &str) -> bool {
    value.to_ascii_lowercase().contains(PLACEHOLDER_MARK)
}

fn fail(path: &std::path::Path, message: &str) -> ! {
    panic!("\n\nkeron.toml ({}): {message}\n\n", path.display());
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let path = manifest.join("../../keron.toml");
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo:rerun-if-env-changed=KERON_ALLOW_PLACEHOLDERS");

    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| fail(&path, &format!("can't read it: {err}")));
    let file: File =
        toml::from_str(&text).unwrap_or_else(|err| fail(&path, &format!("can't parse it: {err}")));

    let host = file.relay.host.trim().to_string();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
        || !host.contains('.')
    {
        fail(
            &path,
            "relay.host must be a lowercase hostname like relay.example.com (no https://, no path)",
        );
    }
    let client_id = file.relay.workos_client_id.trim().to_string();
    if !client_id.starts_with("client_") {
        fail(&path, "relay.workos_client_id must start with client_");
    }
    let team = file.apple.team_id.trim().to_string();
    // The placeholder is let through here and caught by the release check
    // below; a real id is exactly 10 uppercase letters or digits.
    if !is_placeholder(&team)
        && (team.len() != 10
            || !team
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
    {
        fail(
            &path,
            "apple.team_id must be your 10-character team id (uppercase letters and digits)",
        );
    }
    let prefix = file.apple.bundle_prefix.trim().to_string();
    let segments: Vec<&str> = prefix.split('.').collect();
    if segments.len() < 2
        || segments
            .iter()
            .any(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        fail(
            &path,
            "apple.bundle_prefix must be reverse-DNS like com.example",
        );
    }
    let releases_page = file
        .updates
        .releases_page
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !releases_page.starts_with("https://") {
        fail(&path, "updates.releases_page must be an https:// URL");
    }

    let mut placeholders = Vec::new();
    for (key, value) in [
        ("relay.host", &host),
        ("relay.workos_client_id", &client_id),
        ("apple.team_id", &team),
        ("apple.bundle_prefix", &prefix),
        ("updates.releases_page", &releases_page),
    ] {
        if is_placeholder(value) {
            placeholders.push(key);
        }
    }

    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let allowed = std::env::var("KERON_ALLOW_PLACEHOLDERS").as_deref() == Ok("1");
    if !placeholders.is_empty() {
        if release && !allowed {
            fail(
                &path,
                &format!(
                    "release builds need the owner's values, but these are still placeholders: {}. \
                     Fill them in (see docs/guide/relay-and-iphone.md), or set \
                     KERON_ALLOW_PLACEHOLDERS=1 for a test-only build.",
                    placeholders.join(", ")
                ),
            );
        }
        println!(
            "cargo:warning=keron.toml still has placeholders ({}); sync and sign-in stay off in this build",
            placeholders.join(", ")
        );
    }

    let relay_url = format!("https://{host}");
    let macos_bundle_id = format!("{prefix}.keron");
    let ios_bundle_id = format!("{prefix}.keron.ios");
    let releases_url = if file.updates.enabled {
        format!("Some({:?})", format!("{relay_url}/releases"))
    } else {
        "None".to_string()
    };

    let mut out = String::new();
    let mut constant = |doc: &str, name: &str, value: &str| {
        out.push_str(&format!("/// {doc}\npub const {name}: &str = {value:?};\n"));
    };
    constant("Relay hostname (`relay.host`).", "RELAY_HOST", &host);
    constant(
        "Relay base URL, `https://` + [`RELAY_HOST`].",
        "RELAY_URL",
        &relay_url,
    );
    constant(
        "WorkOS AuthKit client id of the relay's app (`relay.workos_client_id`).",
        "WORKOS_CLIENT_ID",
        &client_id,
    );
    constant(
        "Apple Developer team id (`apple.team_id`).",
        "APPLE_TEAM_ID",
        &team,
    );
    constant(
        "Reverse-DNS prefix (`apple.bundle_prefix`).",
        "BUNDLE_PREFIX",
        &prefix,
    );
    constant(
        "macOS app bundle id; also the daemon's launchd label.",
        "MACOS_BUNDLE_ID",
        &macos_bundle_id,
    );
    constant(
        "macOS dev bundle id (scripts/run-macos-dev.sh).",
        "MACOS_DEV_BUNDLE_ID",
        &format!("{macos_bundle_id}.dev"),
    );
    constant(
        "iPhone app bundle id; also the relay's APNS_TOPIC.",
        "IOS_BUNDLE_ID",
        &ios_bundle_id,
    );
    constant(
        "Releases web page (`updates.releases_page`).",
        "RELEASES_PAGE",
        &releases_page,
    );
    constant(
        "The newest release's web page.",
        "LATEST_RELEASE_PAGE",
        &format!("{releases_page}/latest"),
    );
    out.push_str(&format!(
        "/// [`MACOS_BUNDLE_ID`] as a C string, for Objective-C APIs.\n\
         pub const MACOS_BUNDLE_ID_C: &core::ffi::CStr = c{macos_bundle_id:?};\n"
    ));
    out.push_str(&format!(
        "/// Release feed (`{{relay}}/releases`) when `updates.enabled`; `None` turns update checks off.\n\
         pub const RELEASES_URL: Option<&str> = {releases_url};\n"
    ));
    out.push_str(&format!(
        "/// keron.toml keys that still hold placeholders.\n\
         pub const PLACEHOLDERS: &[&str] = &{placeholders:?};\n"
    ));

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("keron_config.rs"), out).expect("writing keron_config.rs");
}
