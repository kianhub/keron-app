//! `keron-config xcconfig` prints the iOS build settings rendered from
//! keron.toml; `keron-config check` reports anything that still disagrees
//! with it or is still a placeholder, and exits 1 if there is any;
//! `keron-config get <key>` prints one derived value.

use keron_config::check::{self, Finding};

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("xcconfig") => print!("{}", check::xcconfig()),
        // For shell scripts (scripts/package-macos.sh, run-macos-dev.sh).
        Some("get") => {
            let value = match std::env::args().nth(2).as_deref() {
                Some("relay-url") => keron_config::RELAY_URL,
                Some("macos-bundle-id") => keron_config::MACOS_BUNDLE_ID,
                Some("macos-dev-bundle-id") => keron_config::MACOS_DEV_BUNDLE_ID,
                Some("ios-bundle-id") => keron_config::IOS_BUNDLE_ID,
                _ => {
                    eprintln!(
                        "usage: keron-config get relay-url | macos-bundle-id | \
                         macos-dev-bundle-id | ios-bundle-id"
                    );
                    std::process::exit(2);
                }
            };
            println!("{value}");
        }
        Some("check") => {
            let findings = check::check(&check::app_root());
            for finding in &findings {
                match finding {
                    Finding::Mismatch(m) => println!("mismatch:    {m}"),
                    Finding::Placeholder(p) => println!("placeholder: {p}"),
                }
            }
            if findings.is_empty() {
                println!(
                    "keron.toml, {} and {} agree; relay {}",
                    check::XCCONFIG_PATH,
                    check::WRANGLER_PATH,
                    keron_config::RELAY_URL
                );
            } else {
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: keron-config xcconfig | check | get <key>");
            std::process::exit(2);
        }
    }
}
