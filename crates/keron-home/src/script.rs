//! Running a `script:<path>` source: a local program that prints the
//! payload as JSON on stdout.
//!
//! It runs directly (no shell), with no stdin, in the widgets folder, with
//! a timeout and an output cap, and is killed if Home drops the fetch. The
//! app passes the login shell's PATH so scripts find uv, node, Homebrew
//! tools and the like.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptOptions {
    pub timeout: Duration,
    /// Most bytes read from stdout.
    pub max_output: usize,
    /// PATH for the script; `None` keeps the app's own.
    pub path_env: Option<String>,
}

impl Default for ScriptOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_output: 1 << 20,
            path_env: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    #[error("no script at {0}")]
    NotFound(String),
    #[error("{0} isn't executable (chmod +x it)")]
    NotExecutable(String),
    #[error("couldn't start the script: {0}")]
    Spawn(String),
    #[error("the script took longer than {0} s")]
    Timeout(u64),
    /// Exit status and the first line of stderr.
    #[error("the script failed ({}): {stderr}", code.map_or("killed".to_string(), |c| format!("exit {c}")))]
    Failed { code: Option<i32>, stderr: String },
    #[error("the script printed more than {0} bytes")]
    TooMuchOutput(usize),
    #[error("the script didn't print JSON: {0}")]
    NotJson(String),
}

/// Most stderr bytes kept (only the first line is shown).
const MAX_STDERR: usize = 64 * 1024;
/// Longest stderr line shown in [`ScriptError::Failed`].
const MAX_STDERR_LINE: usize = 200;

/// Where a manifest's script path points: relative paths are under the
/// widgets folder, `~/` is the home folder. It must exist.
pub fn resolve(widgets_dir: &Path, script: &Path) -> Result<PathBuf, ScriptError> {
    let shown = script.display().to_string();
    let path = match script.strip_prefix("~") {
        Ok(rest) if script.to_str().is_some_and(|s| s.starts_with("~/")) => {
            let home = std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .ok_or_else(|| ScriptError::NotFound(shown.clone()))?;
            PathBuf::from(home).join(rest)
        }
        _ if script.is_relative() => widgets_dir.join(script),
        _ => script.to_path_buf(),
    };
    let metadata = std::fs::metadata(&path).map_err(|_| ScriptError::NotFound(shown.clone()))?;
    if !metadata.is_file() {
        return Err(ScriptError::NotFound(shown));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(ScriptError::NotExecutable(shown));
        }
    }
    Ok(path)
}

/// Run the script and parse its stdout as JSON. Must run inside tokio.
pub async fn run(
    widgets_dir: &Path,
    script: &Path,
    options: &ScriptOptions,
) -> Result<Value, ScriptError> {
    let path = resolve(widgets_dir, script)?;
    let mut command = tokio::process::Command::new(&path);
    command
        .current_dir(widgets_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path_env) = &options.path_env {
        command.env("PATH", path_env);
    }
    let mut child = command
        .spawn()
        .map_err(|e| ScriptError::Spawn(e.to_string()))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(ScriptError::Spawn("no output pipes".to_string()));
    };

    let finished = tokio::time::timeout(options.timeout, async {
        tokio::try_join!(
            async { child.wait().await.map_err(Stop::Io) },
            read_stdout(stdout, options.max_output),
            async { read_stderr(stderr).await.map_err(Stop::Io) },
        )
    })
    .await;
    let (status, stdout, stderr) = match finished {
        Ok(Ok(output)) => output,
        Ok(Err(stop)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(match stop {
                Stop::TooMuch => ScriptError::TooMuchOutput(options.max_output),
                Stop::Io(e) => ScriptError::Spawn(e.to_string()),
            });
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ScriptError::Timeout(options.timeout.as_secs()));
        }
    };

    if !status.success() {
        return Err(ScriptError::Failed {
            code: status.code(),
            stderr: first_line(&stderr),
        });
    }
    serde_json::from_slice(&stdout).map_err(|e| ScriptError::NotJson(e.to_string()))
}

/// Why reading stopped early.
enum Stop {
    TooMuch,
    Io(std::io::Error),
}

/// All of stdout, or [`Stop::TooMuch`] as soon as it passes `limit`.
async fn read_stdout(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, Stop> {
    let mut output = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await.map_err(Stop::Io)?;
        if read == 0 {
            return Ok(output);
        }
        if output.len() + read > limit {
            return Err(Stop::TooMuch);
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

/// The start of stderr; the rest is drained so the script never blocks on
/// a full pipe.
async fn read_stderr(mut reader: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        let keep = read.min(MAX_STDERR.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
    }
}

/// The first non-blank stderr line, trimmed and cut to 200 characters.
fn first_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(MAX_STDERR_LINE).collect()
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        PathBuf::from(name)
    }

    #[tokio::test]
    async fn runs_in_the_widgets_folder_with_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("count.txt"), "3").unwrap();
        let ok = script(
            dir.path(),
            "ok.sh",
            r#"printf '{"value": %s, "label": "%s"}' "$(cat count.txt)" "$PATH""#,
        );
        let options = ScriptOptions {
            path_env: Some("/usr/bin:/bin".to_string()),
            ..ScriptOptions::default()
        };
        let value = run(dir.path(), &ok, &options).await.unwrap();
        assert_eq!(value, json!({"value": 3, "label": "/usr/bin:/bin"}));
    }

    #[tokio::test]
    async fn bad_output_and_failures_are_plain_errors() {
        let dir = tempfile::tempdir().unwrap();
        let options = ScriptOptions::default();

        let text = script(dir.path(), "text.sh", "echo hello");
        assert!(matches!(
            run(dir.path(), &text, &options).await,
            Err(ScriptError::NotJson(_))
        ));

        let long_line = "x".repeat(300);
        let fails = script(
            dir.path(),
            "fails.sh",
            &format!(
                "echo '{{}}'; echo '' >&2; echo '  {long_line}  ' >&2; echo second >&2; exit 3"
            ),
        );
        let Err(ScriptError::Failed { code, stderr }) = run(dir.path(), &fails, &options).await
        else {
            panic!("expected a failure");
        };
        assert_eq!(code, Some(3));
        assert_eq!(stderr, "x".repeat(200));

        let chatty = script(dir.path(), "chatty.sh", "yes '{}'");
        let small = ScriptOptions {
            max_output: 1000,
            ..ScriptOptions::default()
        };
        assert_eq!(
            run(dir.path(), &chatty, &small).await,
            Err(ScriptError::TooMuchOutput(1000))
        );

        std::fs::write(dir.path().join("plain.sh"), "#!/bin/sh\necho '{}'\n").unwrap();
        assert!(matches!(
            run(dir.path(), Path::new("plain.sh"), &options).await,
            Err(ScriptError::NotExecutable(_))
        ));
        assert!(matches!(
            run(dir.path(), Path::new("missing.sh"), &options).await,
            Err(ScriptError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_slow_script_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        // exec, so the pid written is the process Home started.
        let slow = script(dir.path(), "slow.sh", "echo $$ > pid; exec sleep 30");
        let options = ScriptOptions {
            timeout: Duration::from_secs(1),
            ..ScriptOptions::default()
        };
        assert_eq!(
            run(dir.path(), &slow, &options).await,
            Err(ScriptError::Timeout(1))
        );
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the script is still running");
    }
}
