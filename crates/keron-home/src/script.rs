//! Running a `script:<path>` source: a local program that prints the
//! payload as JSON on stdout.
//!
//! It runs directly (no shell), with no stdin, in the widgets folder, with
//! a timeout and an output cap, and is killed if Home drops the fetch. The
//! app passes the login shell's PATH so scripts find uv, node, Homebrew
//! tools and the like.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

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

/// Where a manifest's script path points: relative paths are under the
/// widgets folder, `~/` is the home folder. It must exist.
pub fn resolve(widgets_dir: &Path, script: &Path) -> Result<PathBuf, ScriptError> {
    let _ = (widgets_dir, script);
    todo!("keron-home: script::resolve")
}

/// Run the script and parse its stdout as JSON. Must run inside tokio.
pub async fn run(widgets_dir: &Path, script: &Path, options: &ScriptOptions) -> Result<Value, ScriptError> {
    let _ = (widgets_dir, script, options);
    todo!("keron-home: script::run")
}
