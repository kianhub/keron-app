//! Per-session permission mode.
//!
//! A run skips approvals and the sandbox only when its session explicitly
//! chose full access, which reaches the harness as
//! `sandbox: danger-full-access` (the engine derives it from the session's
//! own config row before dispatch). `auto_approve` alone never grants it.
//! Every other run asks the user before the agent acts: Claude Code's
//! `can_use_tool` requests and Codex's approval requests round-trip through
//! [`crate::RunControls::request_input`] as a yes/no question ([`approve`]).
//!
//! The owner can turn full access on while a run that asks is working. The
//! host follows the session's enforced access live
//! ([`crate::RunControls::access`]): from then on the run's approvals,
//! including any already waiting, are allowed without asking, until the
//! owner turns it off again. Only prompts follow it: the sandbox and the
//! CLI's flags stay as the run started them, so a run that started with full
//! access keeps it until the next turn. The other harnesses never ask, so a
//! switch mid-run has nothing to answer for them.

use tokio::sync::watch;
use zeron_proto::{RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion};

/// Whether this run may act without asking and without a sandbox.
pub fn full_access(request: &RunRequest) -> bool {
    request.sandbox == SandboxLevel::DangerFullAccess
}

/// The session's full access as the host enforces it, followed live while a
/// run is in flight: the host flips it when the owner turns full access on
/// or off mid-run. The default never changes and never has full access (a
/// run nobody follows keeps what it started with).
#[derive(Clone, Debug, Default)]
pub struct LiveAccess(Option<watch::Receiver<bool>>);

impl LiveAccess {
    /// A followed view starting at `full`, and the sender the host flips it
    /// with.
    pub fn channel(full: bool) -> (watch::Sender<bool>, Self) {
        let (tx, rx) = watch::channel(full);
        (tx, Self(Some(rx)))
    }

    /// Whether the session has full access right now.
    pub fn full(&self) -> bool {
        self.0.as_ref().is_some_and(|rx| *rx.borrow())
    }

    /// Resolves once the session has full access, at once when it has it
    /// now. Never resolves once nobody follows the session any more.
    async fn turned_full(&mut self) {
        if let Some(rx) = self.0.as_mut()
            && rx.wait_for(|full| *full).await.is_ok()
        {
            return;
        }
        std::future::pending().await
    }
}

/// The yes/no question an approval request surfaces to the user.
pub fn approval_question(header: String, question: String) -> UserInputQuestion {
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

/// Ask the user to approve one action, and whether they did. A session with
/// full access is approved without asking. A question still waiting when
/// the owner turns full access on is approved then: its wait is dropped,
/// which resolves the pending question through the engine's input bridge
/// just as an answer would (the bridge owns `InputRequested`/`InputResolved`).
///
/// Only approvals go through here. A real question to the user (Claude's
/// `AskUserQuestion`, Codex's tool input requests) is asked directly and
/// always waits for the user.
pub async fn approve(
    request_input: &crate::RequestInput,
    access: &mut LiveAccess,
    question: UserInputQuestion,
) -> bool {
    if access.full() {
        return true;
    }
    let answer = request_input(vec![question.clone()]);
    tokio::select! {
        // The user's own answer wins a tie with the switch.
        biased;
        // A dropped sender (caller went away) is a refusal, never a silent
        // allow.
        answers = answer => approved(&question, &answers.unwrap_or_default()),
        () = access.turned_full() => true,
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
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

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

    type Waiting = Arc<Mutex<Vec<oneshot::Sender<Vec<UserInputAnswer>>>>>;

    /// A user who never answers: every question waits, its answer slot kept.
    fn silent_user() -> (
        Waiting,
        impl Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync,
    ) {
        let waiting: Waiting = Arc::default();
        let slots = waiting.clone();
        (waiting, move |_| {
            let (tx, rx) = oneshot::channel();
            slots.lock().unwrap().push(tx);
            rx
        })
    }

    fn question() -> UserInputQuestion {
        approval_question("Approve command".into(), "Run `make`?".into())
    }

    #[tokio::test]
    async fn approvals_follow_the_session_access_live() {
        let (waiting, ask) = silent_user();
        let (switch, access) = LiveAccess::channel(false);

        // Asking: the approval waits for the user.
        let mut waiter = access.clone();
        let pending = tokio::spawn(async move { approve(&ask, &mut waiter, question()).await });
        tokio::task::yield_now().await;
        while waiting.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        assert!(!pending.is_finished());

        // Turning full access on approves it, and drops the wait so the
        // engine's bridge resolves the user's pending question.
        switch.send(true).unwrap();
        assert!(pending.await.unwrap());
        assert!(waiting.lock().unwrap()[0].is_closed());

        // Later approvals don't ask at all.
        let (waiting, ask) = silent_user();
        assert!(approve(&ask, &mut access.clone(), question()).await);
        assert!(waiting.lock().unwrap().is_empty());

        // Back to asking: the next approval asks, and "No" refuses it.
        switch.send(false).unwrap();
        let ask = |questions: Vec<UserInputQuestion>| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![UserInputAnswer {
                question_id: questions[0].id.clone(),
                labels: vec!["No".into()],
            }]);
            rx
        };
        assert!(!approve(&ask, &mut access.clone(), question()).await);
    }

    #[tokio::test]
    async fn a_run_nobody_follows_keeps_waiting_for_the_user() {
        // The host stopped following the session: that is no approval.
        let (switch, mut access) = LiveAccess::channel(false);
        drop(switch);
        let (_waiting, ask) = silent_user();
        let wait = approve(&ask, &mut access, question());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), wait)
                .await
                .is_err(),
            "still waiting for the user"
        );
    }
}
