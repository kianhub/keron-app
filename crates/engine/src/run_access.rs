//! Per-session full access, enforced on the host.
//!
//! A run skips approvals and the sandbox only when its session chose it: the
//! chat row's config says `danger-full-access`, written by an explicit
//! `setChatConfig` from the session's own controls. A run request can't
//! widen access by itself (a client's `autoApprove` flag, an agent asking
//! for `danger-full-access` through the Zeron MCP tools), and new sessions
//! and forks always start asking. A session that did choose full access gets
//! it on every run, whichever device or client queued the prompt.
//!
//! The host can't see who wrote a row, only the registry's per-field clocks,
//! so "chose it" means the row's `config` was written after the row was
//! minted (`RegistryDoc::chat_config_set_after_mint`). A row minted with
//! full access in it (a phone's or any client's `createSession`, a claimed,
//! imported or re-homed row) runs asking, and the host rewrites it to say so
//! when it adopts or runs it (`WorkspaceHost::settle_session_access`).
//! Every whole-row write re-mints the row, so a session that did choose full
//! access goes back to asking after a re-home (`set_chat_host`), an import
//! (`import_chat_row`) or a backdated activity stamp (`set_chat_activity`).
//!
//! The field clocks are local hybrid clocks with no merge on receive, so
//! "after" depends on the devices' wall clocks. A toggle made on a device
//! whose clock lags the host's when it minted the row doesn't count: the
//! session keeps asking, and the host rewrites the row so that device's
//! controls show "Ask first" again. This fails safe; choosing full access
//! again once the clocks agree takes effect.
//!
//! A run in flight follows the same enforced view live ([`follow`]): when
//! the owner turns full access on mid-turn, a harness that asks approves
//! what's waiting and stops asking for the rest of the run
//! (`zeron_harness::permissions::approve`). Its sandbox and CLI flags stay
//! as the run started them; those change from the next turn.

use tokio::sync::watch;
use zeron_proto::{ChatConfig, RunRequest, SandboxLevel};

use crate::workspace_host::WorkspaceHost;

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

/// Keep a live run's view of its session's full access (`access`) in step
/// with the host's enforced row ([`WorkspaceHost::session_config`]) on every
/// registry change, whichever device made it. Ends once the run lets go of
/// its view: a harness that never asks drops it at once.
pub(crate) fn follow(workspace: WorkspaceHost, chat_id: String, access: watch::Sender<bool>) {
    // Subscribed before the first read, so no change slips between them.
    let mut changes = workspace.watch_changes();
    tokio::spawn(async move {
        loop {
            let full = workspace
                .session_config(&chat_id)
                .is_some_and(|c| c.sandbox == SandboxLevel::DangerFullAccess);
            access.send_if_modified(|current| std::mem::replace(current, full) != full);
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { break },
                () = access.closed() => break,
            }
        }
    });
}

/// A new session (or a fork) starts asking: full access is turned on
/// afterwards, on the session itself.
pub(crate) fn start_asking(config: &mut ChatConfig) {
    if config.sandbox == SandboxLevel::DangerFullAccess {
        config.sandbox = SandboxLevel::WorkspaceWrite;
    }
}

/// A session row's config as the host enforces it. `chosen_later`: the
/// config was written after the row was minted (see the module docs); full
/// access minted with the row starts asking instead.
pub(crate) fn enforced(mut config: ChatConfig, chosen_later: bool) -> ChatConfig {
    if !chosen_later {
        start_asking(&mut config);
    }
    config
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
    fn only_full_access_chosen_after_the_row_was_minted_counts() {
        let full = config(SandboxLevel::DangerFullAccess);
        assert_eq!(
            enforced(full.clone(), true).sandbox,
            SandboxLevel::DangerFullAccess
        );
        assert_eq!(enforced(full, false).sandbox, SandboxLevel::WorkspaceWrite);
        for sandbox in [SandboxLevel::ReadOnly, SandboxLevel::WorkspaceWrite] {
            assert_eq!(enforced(config(sandbox), false).sandbox, sandbox);
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
