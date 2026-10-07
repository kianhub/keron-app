//! Cross-file checks: the iOS xcconfig and the relay's wrangler.jsonc must
//! agree with keron.toml (they're read by Xcode and wrangler, which can't
//! read keron.toml themselves).

use std::path::{Path, PathBuf};

use crate::{APPLE_TEAM_ID, IOS_BUNDLE_ID, PLACEHOLDERS, RELAY_HOST, WORKOS_CLIENT_ID};

/// Where the iOS project reads its build settings from.
pub const XCCONFIG_PATH: &str = "apps/ios/Keron.xcconfig";
/// The relay Worker's config.
pub const WRANGLER_PATH: &str = "edge/wrangler.jsonc";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A file disagrees with keron.toml.
    Mismatch(String),
    /// A value is still a placeholder: fine for a debug build, not for deploying.
    Placeholder(String),
}

/// The app root this crate was built from.
pub fn app_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The iOS build settings, rendered from keron.toml.
pub fn xcconfig() -> String {
    format!(
        "// Generated from keron.toml by\n\
         //   cargo run -p keron-config --features cli -- xcconfig > apps/ios/Keron.xcconfig\n\
         // Edit keron.toml, not this file.\n\
         KERON_APPLE_TEAM_ID = {APPLE_TEAM_ID}\n\
         KERON_IOS_BUNDLE_ID = {IOS_BUNDLE_ID}\n"
    )
}

/// Every disagreement or leftover placeholder across keron.toml, the
/// xcconfig and wrangler.jsonc under `root`.
pub fn check(root: &Path) -> Vec<Finding> {
    let mut findings: Vec<Finding> = PLACEHOLDERS
        .iter()
        .map(|key| Finding::Placeholder(format!("keron.toml {key}")))
        .collect();

    match std::fs::read_to_string(root.join(XCCONFIG_PATH)) {
        Ok(text) if text == xcconfig() => {}
        Ok(_) => findings.push(Finding::Mismatch(format!(
            "{XCCONFIG_PATH} is out of date; regenerate it from keron.toml"
        ))),
        Err(err) => findings.push(Finding::Mismatch(format!("{XCCONFIG_PATH}: {err}"))),
    }

    match std::fs::read_to_string(root.join(WRANGLER_PATH)) {
        Ok(text) => match json5::from_str::<serde_json::Value>(&text) {
            Ok(wrangler) => check_wrangler(&wrangler, &mut findings),
            Err(err) => findings.push(Finding::Mismatch(format!("{WRANGLER_PATH}: {err}"))),
        },
        Err(err) => findings.push(Finding::Mismatch(format!("{WRANGLER_PATH}: {err}"))),
    }
    findings
}

fn check_wrangler(wrangler: &serde_json::Value, findings: &mut Vec<Finding>) {
    let text = |pointer: &str| wrangler.pointer(pointer).and_then(|v| v.as_str());
    let mut expect = |pointer: &str, want: &str| match text(pointer) {
        Some(got) if got == want => {}
        got => findings.push(Finding::Mismatch(format!(
            "{WRANGLER_PATH} {pointer} is {got:?}, keron.toml says {want:?}"
        ))),
    };
    expect("/name", "keron-edge");
    expect("/vars/AUTH_MODE", "workos");
    expect("/vars/WORKOS_CLIENT_ID", WORKOS_CLIENT_ID);
    expect("/vars/APNS_TEAM_ID", APPLE_TEAM_ID);
    expect("/vars/APNS_TOPIC", IOS_BUNDLE_ID);

    let routes = wrangler
        .pointer("/routes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let relay_route = routes.iter().any(|route| {
        route.get("pattern").and_then(|v| v.as_str()) == Some(RELAY_HOST)
            && route.get("custom_domain").and_then(|v| v.as_bool()) == Some(true)
    });
    if !relay_route {
        findings.push(Finding::Mismatch(format!(
            "{WRANGLER_PATH} routes need {{ \"pattern\": \"{RELAY_HOST}\", \"custom_domain\": true }}"
        )));
    }
    for route in &routes {
        let pattern = route.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.contains("zeron") {
            findings.push(Finding::Mismatch(format!(
                "{WRANGLER_PATH} still routes {pattern}"
            )));
        }
    }

    let buckets = wrangler
        .pointer("/r2_buckets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for bucket in &buckets {
        let name = bucket
            .get("bucket_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !name.starts_with("keron-") {
            findings.push(Finding::Mismatch(format!(
                "{WRANGLER_PATH} R2 bucket {name:?} should be named keron-*"
            )));
        }
    }

    match text("/account_id") {
        Some(id) if id.to_ascii_lowercase().contains("change-me") => {
            findings.push(Finding::Placeholder(format!("{WRANGLER_PATH} account_id")))
        }
        Some(_) => {}
        None => findings.push(Finding::Mismatch(format!(
            "{WRANGLER_PATH} needs your Cloudflare account_id"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed xcconfig and wrangler.jsonc agree with keron.toml.
    /// Placeholders are fine here; deploying needs `keron-config check` clean.
    #[test]
    fn committed_files_agree_with_keron_toml() {
        let mismatches: Vec<_> = check(&app_root())
            .into_iter()
            .filter(|f| matches!(f, Finding::Mismatch(_)))
            .collect();
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn wrangler_mismatches_are_reported() {
        let wrangler = serde_json::json!({
            "name": "comet-native-edge",
            "account_id": "abc",
            "routes": [{ "pattern": "edge.zeron.sh", "custom_domain": true }],
            "r2_buckets": [{ "binding": "BLOBS", "bucket_name": "comet-native-blobs" }],
            "vars": { "AUTH_MODE": "dev", "WORKOS_CLIENT_ID": "client_other" }
        });
        let mut findings = Vec::new();
        check_wrangler(&wrangler, &mut findings);
        let text = format!("{findings:?}");
        for needle in [
            "/name",
            "AUTH_MODE",
            "WORKOS_CLIENT_ID",
            "APNS_TEAM_ID",
            "APNS_TOPIC",
            "routes need",
            "edge.zeron.sh",
            "comet-native-blobs",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }
}
