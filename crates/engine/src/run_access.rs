//! Per-session full access, enforced on the host.
//!
//! A run skips approvals and the sandbox only when its session chose it: the
//! chat row's config says `danger-full-access`, written by an explicit
//! `setChatConfig` from the session's own controls. A run request can't
//! widen access by itself (a client's `autoApprove` flag, an agent asking
//! for `danger-full-access` through the Zeron MCP tools), and new sessions
//! and forks always start asking. A session that did choose full access gets
//! it on every run, whichever device or client queued the prompt.

use zeron_proto::{ChatConfig, RunRequest, SandboxLevel};

/// Fit `request`'s sandbox and auto-approve flag to the session's choice.
/// `session` is the chat row's config; a missing row or config asks.
pub(crate) fn apply(request: &mut RunRequest, session: Option<&ChatConfig>) {
    let full = session.is_some_and(|c| c.sandbox == SandboxLevel::DangerFullAccess);
    if full {
        request.sandbox = SandboxLevel::DangerFullAccess;
    } else if request.sandbox == SandboxLevel::DangerFullAccess {
        request.sandbox = SandboxLevel::WorkspaceWrite;
    }
    request.auto_approve = full;
}

/// A new session (or a fork) starts asking: full access is turned on
/// afterwards, on the session itself.
pub(crate) fn start_asking(config: &mut ChatConfig) {
    if config.sandbox == SandboxLevel::DangerFullAccess {
        config.sandbox = SandboxLevel::WorkspaceWrite;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::HarnessId;

    fn request(sandbox: SandboxLevel, auto_approve: bool) -> RunRequest {
        RunRequest {
            prompt: String::new(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: String::new(),
            sandbox,
            auto_approve,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
            mcp: None,
        }
    }

    fn config(sandbox: SandboxLevel) -> ChatConfig {
        ChatConfig {
            harness: HarnessId::ClaudeCode,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            sandbox,
        }
    }

    #[test]
    fn a_request_cannot_widen_a_session_that_asks() {
        for session in [
            None,
            Some(config(SandboxLevel::WorkspaceWrite)),
            Some(config(SandboxLevel::ReadOnly)),
        ] {
            let mut req = request(SandboxLevel::DangerFullAccess, true);
            apply(&mut req, session.as_ref());
            assert_eq!(req.sandbox, SandboxLevel::WorkspaceWrite);
            assert!(!req.auto_approve);
        }
        // A narrower request keeps its own sandbox.
        let mut req = request(SandboxLevel::ReadOnly, true);
        apply(&mut req, Some(&config(SandboxLevel::WorkspaceWrite)));
        assert_eq!(req.sandbox, SandboxLevel::ReadOnly);
        assert!(!req.auto_approve);
    }

    #[test]
    fn a_full_access_session_gets_it_on_every_run() {
        for (sandbox, auto_approve) in [
            (SandboxLevel::WorkspaceWrite, false),
            (SandboxLevel::ReadOnly, true),
            (SandboxLevel::DangerFullAccess, false),
        ] {
            let mut req = request(sandbox, auto_approve);
            apply(&mut req, Some(&config(SandboxLevel::DangerFullAccess)));
            assert_eq!(req.sandbox, SandboxLevel::DangerFullAccess);
            assert!(req.auto_approve);
        }
    }

    #[test]
    fn new_sessions_start_asking() {
        let mut full = config(SandboxLevel::DangerFullAccess);
        start_asking(&mut full);
        assert_eq!(full.sandbox, SandboxLevel::WorkspaceWrite);
        let mut read_only = config(SandboxLevel::ReadOnly);
        start_asking(&mut read_only);
        assert_eq!(read_only.sandbox, SandboxLevel::ReadOnly);
    }
}
