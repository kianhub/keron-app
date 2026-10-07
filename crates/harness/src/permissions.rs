//! Per-session permission mode.
//!
//! A run skips approvals and the sandbox only when its session explicitly
//! chose full access, which reaches the harness as
//! `sandbox: danger-full-access` (the engine derives it from the session's
//! own config row before dispatch). `auto_approve` alone never grants it.
//! Every other run asks the user before the agent acts: Claude Code's
//! `can_use_tool` requests and Codex's approval requests round-trip through
//! [`crate::RunControls::request_input`] as a yes/no question.

use zeron_proto::{RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion};

/// Whether this run may act without asking and without a sandbox.
pub fn full_access(request: &RunRequest) -> bool {
    request.sandbox == SandboxLevel::DangerFullAccess
}

/// The yes/no question an approval request surfaces to the user.
pub(crate) fn approval_question(header: String, question: String) -> UserInputQuestion {
    UserInputQuestion {
        id: uuid::Uuid::new_v4().to_string(),
        header,
        question,
        options: vec!["Yes".into(), "No".into()],
        prefill: None,
        multiline: false,
        multi_select: false,
    }
}

/// Whether the user answered "Yes" to `question`. Anything else, including
/// no answer at all (the caller went away), is a refusal: an approval is
/// never granted by default.
pub(crate) fn approved(question: &UserInputQuestion, answers: &[UserInputAnswer]) -> bool {
    answers.iter().any(|a| {
        a.question_id == question.id && a.labels.iter().any(|l| l.eq_ignore_ascii_case("yes"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn only_danger_full_access_is_full_access() {
        assert!(full_access(&request(SandboxLevel::DangerFullAccess, true)));
        assert!(full_access(&request(SandboxLevel::DangerFullAccess, false)));
        // A client's auto_approve flag never widens a sandboxed session.
        assert!(!full_access(&request(SandboxLevel::WorkspaceWrite, true)));
        assert!(!full_access(&request(SandboxLevel::ReadOnly, true)));
        assert!(!full_access(&request(SandboxLevel::WorkspaceWrite, false)));
    }

    #[test]
    fn only_an_explicit_yes_approves() {
        let q = approval_question("Approve command".into(), "Run `ls`?".into());
        let answer = |id: &str, label: &str| UserInputAnswer {
            question_id: id.into(),
            labels: vec![label.into()],
        };
        assert!(approved(&q, &[answer(&q.id, "Yes")]));
        assert!(approved(&q, &[answer(&q.id, "yes")]));
        assert!(!approved(&q, &[answer(&q.id, "No")]));
        assert!(!approved(&q, &[answer("other", "Yes")]));
        assert!(!approved(&q, &[]));
    }
}
