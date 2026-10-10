//! Git fixtures that behave the same on every machine.
//!
//! A developer's global git config can turn on commit signing (an SSH key
//! held by 1Password, say), hooks, a commit template or a different default
//! branch, any of which makes a throwaway commit in a fixture repository
//! fail or stall. Fixture commands therefore read no global or system config
//! and bring their own identity. `/dev/null` reads as an empty config on
//! every platform git runs on, Windows included, where git maps it to `nul`.
use std::path::Path;

/// `git <args>` in `cwd`, isolated from the developer's git setup.
pub fn command(cwd: &Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test");
    command
}
