//! ClaudeHarness integration tests against the fake CLI in
//! `tests/fixtures/fake-claude.sh` (no real `claude` binary involved).
//! A live smoke test against the real CLI lives at the bottom, `#[ignore]`d.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    CancellationToken, ClaudeHarness, Harness, HarnessError, LiveAccess, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall, UserInputAnswer,
    UserInputQuestion,
};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-claude.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

fn harness() -> ClaudeHarness {
    ClaudeHarness::new().with_executable(fixture_path())
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

/// Controls whose `request_input` answers every question with `answer_label`.
fn controls(
    answer_label: &'static str,
) -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec![answer_label.into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        access: Default::default(),
    };
    (controls, steer_tx, token)
}

async fn run_to_end(
    harness: &ClaudeHarness,
    req: RunRequest,
    controls: RunControls,
) -> Vec<AgentEvent> {
    let stream = harness.run(req, controls).await.expect("run starts");
    tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time")
}

#[tokio::test]
async fn happy_path_normalizes_events_and_tags_subagents() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:happy"), controls).await;

    // One SessionStarted despite the re-emitted init frame.
    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionStarted {
                harness,
                model,
                tools,
                session_id,
                ..
            } => Some((harness, model, tools, session_id)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "init must be deduped: {events:?}");
    let (h, model, tools, session_id) = starts[0];
    assert_eq!(*h, HarnessId::ClaudeCode);
    assert_eq!(model, "claude-fable-5");
    assert_eq!(tools, &vec!["Bash".to_string(), "Read".to_string()]);
    assert_eq!(session_id, "sess-1");

    assert!(events.contains(&AgentEvent::ReasoningDelta {
        text: "pondering".into()
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "Hello".into()
    }));

    // Subagent frames (parent_tool_use_id set) arrive TAGGED — never as bare
    // parent-feed events.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::TextDelta { text } if text.contains("SUBAGENT")
        )),
        "subagent delta leaked into the parent feed: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCall { id, .. } | AgentEvent::ToolResult { id, .. } if id == "sub-tool"
        )),
        "subagent tool frames leaked into the parent feed: {events:?}"
    );
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::TextDelta {
            text: "SUBAGENT".into()
        }),
    }));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::ToolCall {
            id: "sub-tool".into(),
            call: ToolCall::Exec {
                command: "echo sub".into()
            },
        }),
    }));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::ToolResult {
            id: "sub-tool".into(),
            is_error: false,
            output: None,
            diff: None,
        }),
    }));

    // Typed tool decoding: Bash -> Exec, mcp__server__tool -> Mcp.
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "tool-1".into(),
        call: ToolCall::Exec {
            command: "ls -la".into()
        },
    }));
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "tool-2".into(),
        call: ToolCall::Mcp {
            server: "linear".into(),
            tool: "search".into(),
            input: Some(serde_json::json!({"q": "bug"})),
        },
    }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantMessageCompleted { .. }))
    );
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "tool-1".into(),
        is_error: false,
        output: None,
        diff: None,
    }));
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "tool-2".into(),
        is_error: true,
        output: None,
        diff: None,
    }));

    // Informational rate-limit frames stay quiet.
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));

    assert!(events.contains(&AgentEvent::Usage {
        input_tokens: 10,
        output_tokens: 20
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("done!".into()),
            error: None,
            session_id: Some("sess-1".into()),
        })
    );
}

#[tokio::test]
async fn eager_done_forwards_wake_turn_as_second_done() {
    // The background-subagent shape (live-verified 2.1.228): result #1 is
    // eager — the run must NOT hold the turn for the subagent — and the wake
    // turn's frames flow through the SAME stream, settling with result #2.
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:wake"), controls).await;

    let done_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AgentEvent::Done { .. }).then_some(i))
        .collect();
    assert_eq!(
        done_positions.len(),
        2,
        "eager done + wake done: {events:?}"
    );

    // One SessionStarted total — the wake init is deduped.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            .count(),
        1
    );

    // The subagent's interior streams tagged BETWEEN the two dones.
    let tagged_position = events
        .iter()
        .position(|e| {
            matches!(e, AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == "toolu_agent")
        })
        .expect("tagged subagent traffic present");
    assert!(
        done_positions[0] < tagged_position && tagged_position < done_positions[1],
        "subagent interior must stream between the eager done and the wake done: {events:?}"
    );

    // The wake turn's own (untagged) output precedes the second done.
    let wake_text = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "subagent finished"))
        .expect("wake-turn delta present");
    assert!(done_positions[0] < wake_text && wake_text < done_positions[1]);

    // Both dones settle Completed with the same session id.
    for i in done_positions {
        assert!(matches!(
            &events[i],
            AgentEvent::Done {
                status: DoneStatus::Completed,
                session_id: Some(id),
                ..
            } if id == "sess-wake"
        ));
    }
}

/// Controls whose `request_input` records every question and answers
/// `answer_label`.
fn recording_controls(
    answer_label: &'static str,
) -> (
    RunControls,
    Arc<Mutex<Vec<UserInputQuestion>>>,
    mpsc::Sender<SteerMessage>,
) {
    let asked: Arc<Mutex<Vec<UserInputQuestion>>> = Arc::new(Mutex::new(Vec::new()));
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let seen = asked.clone();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            seen.lock().unwrap().extend(questions.iter().cloned());
            let (tx, rx) = oneshot::channel();
            let answers = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec![answer_label.into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: CancellationToken::new(),
        access: Default::default(),
    };
    (controls, asked, steer_tx)
}

fn final_result(events: &[AgentEvent]) -> Option<String> {
    events.iter().rev().find_map(|e| match e {
        AgentEvent::Done { result, .. } => result.clone(),
        _ => None,
    })
}

#[tokio::test]
async fn tool_calls_ask_the_user_unless_the_session_chose_full_access() {
    // A sandboxed session asks, even when a client sets auto_approve.
    let mut req = request("scenario:tool-approval");
    req.sandbox = SandboxLevel::WorkspaceWrite;
    req.auto_approve = true;

    let (controls, asked, _steer) = recording_controls("Yes");
    let events = run_to_end(&harness(), req.clone(), controls).await;
    assert_eq!(
        final_result(&events).as_deref(),
        Some("allowed bypass=no"),
        "{events:?}"
    );
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].header, "Approve command");
    assert!(asked[0].question.contains("rm -rf build"));
    assert_eq!(asked[0].options, vec!["Yes".to_string(), "No".to_string()]);

    let (controls, _asked, _steer) = recording_controls("No");
    let events = run_to_end(&harness(), req, controls).await;
    assert_eq!(
        final_result(&events).as_deref(),
        Some("denied bypass=no"),
        "{events:?}"
    );

    // Full access: the CLI runs with the bypass flag and nothing is asked.
    let mut req = request("scenario:tool-approval");
    req.sandbox = SandboxLevel::DangerFullAccess;
    let (controls, asked, _steer) = recording_controls("No");
    let events = run_to_end(&harness(), req, controls).await;
    assert_eq!(
        final_result(&events).as_deref(),
        Some("allowed bypass=yes"),
        "{events:?}"
    );
    assert!(asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unanswered_approval_is_a_denial() {
    // The engine's input bridge went away without answering (the viewer
    // closed, the run was torn down): the tool is denied, never allowed.
    let mut req = request("scenario:tool-approval");
    req.sandbox = SandboxLevel::WorkspaceWrite;
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let _steer = steer_tx;
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| oneshot::channel().1),
        steering: steer_rx,
        interrupt: CancellationToken::new(),
        access: Default::default(),
    };
    let events = run_to_end(&harness(), req, controls).await;
    assert_eq!(
        final_result(&events).as_deref(),
        Some("denied bypass=no"),
        "{events:?}"
    );
}

#[tokio::test]
async fn approvals_and_agent_questions_interleave_in_a_session_that_asks() {
    // Asking mode: the Bash call becomes a yes/no approval, then the
    // AskUserQuestion still reaches the user as its own question.
    let mut req = request("scenario:askuser");
    req.sandbox = SandboxLevel::WorkspaceWrite;
    let asked: Arc<Mutex<Vec<UserInputQuestion>>> = Arc::new(Mutex::new(Vec::new()));
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let _steer = steer_tx;
    let seen = asked.clone();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            seen.lock().unwrap().extend(questions.iter().cloned());
            let (tx, rx) = oneshot::channel();
            let answers = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec![if q.options.iter().any(|o| o == "Yes") {
                        "Yes".into()
                    } else {
                        "B".into()
                    }],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: CancellationToken::new(),
        access: Default::default(),
    };
    let events = run_to_end(&harness(), req, controls).await;
    let asked = asked.lock().unwrap().clone();
    let headers: Vec<_> = asked.iter().map(|q| q.header.as_str()).collect();
    assert_eq!(headers, ["Approve command", "Choice"], "{asked:?}");
    assert_eq!(
        final_result(&events).as_deref(),
        Some("answered"),
        "{events:?}"
    );
}

/// Every question asked so far, each with its answer slot: the test answers
/// by hand, or never.
type Waiting = Arc<Mutex<Vec<(UserInputQuestion, oneshot::Sender<Vec<UserInputAnswer>>)>>>;

/// Controls whose `request_input` answers nothing by itself.
fn waiting_controls(access: LiveAccess) -> (RunControls, Waiting, mpsc::Sender<SteerMessage>) {
    let waiting: Waiting = Arc::default();
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let slots = waiting.clone();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |mut questions| {
            assert_eq!(questions.len(), 1, "{questions:?}");
            let (tx, rx) = oneshot::channel();
            slots.lock().unwrap().push((questions.remove(0), tx));
            rx
        }),
        steering: steer_rx,
        interrupt: CancellationToken::new(),
        access,
    };
    (controls, waiting, steer_tx)
}

/// The questions asked once there are `count` of them.
async fn asked(waiting: &Waiting, count: usize) -> Vec<UserInputQuestion> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let waiting = waiting.lock().unwrap();
                if waiting.len() >= count {
                    return waiting.iter().map(|(q, _)| q.clone()).collect();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("questions asked in time")
}

#[tokio::test]
async fn full_access_turned_on_mid_run_allows_the_waiting_and_later_tool_calls() {
    let mut req = request("scenario:live-access");
    req.sandbox = SandboxLevel::WorkspaceWrite;
    req.auto_approve = false;
    let (switch, access) = LiveAccess::channel(false);
    let (controls, waiting, _steer) = waiting_controls(access);
    let stream = harness().run(req, controls).await.expect("run starts");
    let events = tokio::spawn(stream.map(|r| r.expect("stream event")).collect::<Vec<_>>());

    // The Bash call waits for the user until the owner turns full access on.
    assert_eq!(asked(&waiting, 1).await[0].header, "Approve command");
    switch.send(true).unwrap();

    // The Write call after it is allowed without asking; AskUserQuestion
    // still reaches the user.
    let headers: Vec<_> = asked(&waiting, 2)
        .await
        .into_iter()
        .map(|q| q.header)
        .collect();
    assert_eq!(headers, ["Approve command", "Choice"]);
    let (choice, answer) = {
        let mut waiting = waiting.lock().unwrap();
        // The approval stopped waiting, which resolves the user's pending
        // question in the engine's input bridge.
        assert!(waiting[0].1.is_closed());
        waiting.remove(1)
    };
    // AskUserQuestion is never answered for the user.
    answer
        .send(vec![UserInputAnswer {
            question_id: choice.id,
            labels: vec!["B".into()],
        }])
        .unwrap();

    let events = tokio::time::timeout(Duration::from_secs(10), events)
        .await
        .expect("run finished in time")
        .unwrap();
    assert_eq!(
        final_result(&events).as_deref(),
        Some("waiting=allowed next=allowed picked=B"),
        "{events:?}"
    );
}

#[tokio::test]
async fn ask_user_question_round_trips_through_the_control_channel() {
    // The questions must reach the ENGINE's input bridge (`request_input`) —
    // and the harness must NOT emit its own `InputRequested`/`InputResolved`
    // twins: the bridge owns that lifecycle (it mints the request id the
    // resolver is parked under; a harness-emitted copy folded an unanswerable
    // duplicate chip into the doc).
    let asked: Arc<Mutex<Vec<UserInputQuestion>>> = Arc::new(Mutex::new(Vec::new()));
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let _steer = steer_tx;
    let token = CancellationToken::new();
    let seen = asked.clone();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            seen.lock().unwrap().extend(questions.iter().cloned());
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec!["B".into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        access: Default::default(),
    };
    let events = run_to_end(&harness(), request("scenario:askuser"), controls).await;

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].header, "Choice");
    assert_eq!(asked[0].question, "Pick one");
    assert_eq!(asked[0].options, vec!["A".to_string(), "B".to_string()]);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::InputRequested { .. } | AgentEvent::InputResolved { .. }
        )),
        "harness must not emit input lifecycle events itself: {events:?}"
    );

    // "answered" proves both control round-trips: the plain Bash can_use_tool
    // was auto-allowed AND the answers reached the CLI as updatedInput.answers
    // keyed by question text.
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("answered".into()),
            error: None,
            session_id: Some("sess-ask".into()),
        })
    );
}

#[tokio::test]
async fn ultrathink_preserves_selected_commands_on_initial_and_steered_sends() {
    use zeron_proto::ReasoningLevel;
    use zeron_proto::invocation::{Invocation, SkillCommand, harness_prompt};

    let invocations = [
        Invocation::Command {
            name: "review".into(),
        },
        Invocation::Skill {
            name: "review".into(),
            path: "/repo/.claude/skills/review/SKILL.md".into(),
            command: Some(SkillCommand {
                name: "review".into(),
                harness: HarnessId::ClaudeCode,
            }),
        },
    ];
    for invocation in invocations {
        for prefix in ["", " ", "   ", "\n", "\r\n  "] {
            let prompt = harness_prompt(
                &format!("{prefix}{} scenario:command-echo", invocation.link()),
                HarnessId::ClaudeCode,
            );
            let expected = format!("{prefix}/review scenario:command-echo");
            let (initial_controls, _steer, _token) = controls("A");
            let mut initial = request(&prompt);
            initial.reasoning = Some(ReasoningLevel::Ultrathink);
            let events = run_to_end(&harness(), initial, initial_controls).await;
            assert!(
                events.contains(&AgentEvent::TextDelta {
                    text: expected.clone()
                }),
                "{events:?}"
            );

            let (steer_controls, steer, _token) = controls("A");
            steer
                .send(SteerMessage {
                    prompt,
                    message_id: None,
                })
                .await
                .unwrap();
            let mut initial = request("scenario:steer");
            initial.reasoning = Some(ReasoningLevel::Ultrathink);
            let events = run_to_end(&harness(), initial, steer_controls).await;
            assert!(
                events.contains(&AgentEvent::TextDelta {
                    text: format!("steered:{expected}")
                }),
                "{events:?}"
            );
        }
    }
}

#[tokio::test]
async fn steering_lines_are_written_to_stdin_mid_run() {
    let (controls, steer, _token) = controls("A");
    steer
        .send(SteerMessage {
            prompt: "redirect please".into(),
            message_id: None,
        })
        .await
        .expect("steer queued");
    let events = run_to_end(&harness(), request("scenario:steer"), controls).await;

    let steered = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Steered {
                assistant_message_id,
                next_assistant_message_id,
            } => Some((
                assistant_message_id.clone(),
                next_assistant_message_id.clone(),
            )),
            _ => None,
        })
        .expect("Steered emitted");
    let boundary = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .unwrap();
    let continuation = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "-still-first"))
        .unwrap();
    assert!(
        continuation < boundary,
        "sending input must not split unconsumed response text"
    );
    assert!(steered.0.is_some() && steered.1.is_some());
    assert_ne!(steered.0, steered.1);

    // The fake CLI echoes the steer line's content back as a delta.
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "steered:redirect please".into()
    }));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn interrupt_escalates_to_sigterm_and_ends_with_interrupted_done() {
    let harness = ClaudeHarness::new()
        .with_executable(fixture_path())
        .with_graces(Duration::from_millis(100), Duration::from_millis(500));
    let (controls, _steer, token) = controls("A");
    let mut stream = harness
        .run(request("scenario:interrupt"), controls)
        .await
        .expect("run starts");

    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::SessionStarted { .. }) {
                token.cancel(); // interrupt as soon as the session is up
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("interrupt completed in time");

    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Interrupted,
            result: None,
            error: None,
            session_id: Some("sess-int".into()),
        })
    );
}

#[tokio::test]
async fn error_codes_map_to_readable_messages() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:error"), controls).await;

    let errors: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        errors.contains(&"Claude usage limit reached — try again after the limit resets."),
        "assistant error code not mapped: {errors:?}"
    );
    assert!(
        errors.contains(
            &"Claude 5-hour limit reached — the turn was blocked. Try again after it resets."
        ),
        "rejected rate_limit_event not mapped: {errors:?}"
    );

    // Empty `errors` array on the result falls back to subtype wording.
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Errored,
            result: None,
            error: Some("The run hit the maximum number of turns.".into()),
            session_id: Some("sess-err".into()),
        })
    );
}

#[tokio::test]
async fn missing_binary_is_not_installed() {
    let harness = ClaudeHarness::new().with_executable("/nonexistent/claude-nowhere");
    let (controls, _steer, _token) = controls("A");
    let err = harness
        .run(request("scenario:happy"), controls)
        .await
        .err()
        .expect("spawn fails");
    assert!(matches!(err, HarnessError::NotInstalled(_)), "{err:?}");
}

#[tokio::test]
async fn captured_live_background_subagent_frames_replay_correctly() {
    // Frames captured VERBATIM from claude 2.1.228 (2026-08-17): a turn that
    // spawns a background Agent subagent — eager result while it runs, tagged
    // subagent traffic, then the wake turn (second init, same session id,
    // second result). Replayed through the fake-CLI transport so the whole
    // driver path (wire parse → normalize → run loop) is exercised, not just
    // the normalizer.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claude")
        .join("live-2.1.228-background-subagent.jsonl");
    let script = std::fs::read_to_string(&fixture).expect("fixture readable");
    // A one-off cat-style fake CLI: reads the prompt line, plays the capture.
    // The capture contains one can_use_tool control_request; the driver
    // auto-allows it on stdin, which this replayer ignores.
    let dir = std::env::temp_dir().join(format!("claude-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let frames = dir.join("frames.jsonl");
    std::fs::write(&frames, &script).expect("frames written");
    let cli = dir.join("replay.sh");
    std::fs::write(
        &cli,
        format!(
            "#!/bin/sh\nread -r _first || exit 1\ncat '{}'\n",
            frames.display()
        ),
    )
    .expect("replayer written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let harness = ClaudeHarness::new().with_executable(&cli);
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness, request("replay"), controls).await;

    // One SessionStarted (the wake init dedupes), two Dones (eager + wake).
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            .count(),
        1,
        "{events:?}"
    );
    let dones: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AgentEvent::Done { .. }).then_some(i))
        .collect();
    assert_eq!(dones.len(), 2, "eager done + wake done: {events:?}");

    // The parent-feed Agent spawn is a plain tool call; the subagent's own
    // Bash call arrives tagged with the spawning tool-use id, between the
    // two dones, and never as a bare parent event.
    let spawn_id = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall { id, call } => match call {
                ToolCall::Unknown { name, .. } if name.starts_with("Agent") => Some(id.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("Agent spawn tool call in the parent feed");
    // The synthesized opening user message rides WITH the spawn (before the
    // eager done); the child's own interior streams between the two dones.
    let opening: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == spawn_id
                        && matches!(event.as_ref(), AgentEvent::UserMessage { .. })
            )
            .then_some(i)
        })
        .collect();
    assert_eq!(opening.len(), 1, "one seeded opening prompt: {events:?}");
    assert!(opening[0] < dones[0], "opening rides with the spawn");
    let tagged: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == spawn_id
                        && !matches!(event.as_ref(), AgentEvent::UserMessage { .. })
            )
            .then_some(i)
        })
        .collect();
    assert!(!tagged.is_empty(), "tagged subagent traffic: {events:?}");
    assert!(
        tagged.iter().all(|i| dones[0] < *i && *i < dones[1]),
        "subagent interior streams between the eager and wake dones"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Subagent { event, .. }
                if matches!(event.as_ref(), AgentEvent::ToolCall { call: ToolCall::Exec { .. }, .. })
        )),
        "subagent Bash call arrives tagged: {events:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Frames captured from claude 2.1.296 (2026-10-09; the init frame's home
/// path is renamed and the plan-usage `rate_limit_event` dropped): a `now`
/// steer lands while the first reply streams. The CLI ends that turn with a
/// `success` result (`aborted_streaming`), replays the steer, re-runs the
/// tool (`sleep 8`, five quiet seconds on stdout), and answers. The replay
/// pauses at that quiet stretch for longer than the harness's held turn end
/// waits: the result held for the steer must not surface there as a Done in
/// the middle of the turn, where the engine parks the chat and drops the
/// reply streaming right after it.
#[tokio::test]
async fn captured_live_mid_turn_steer_keeps_the_whole_reply() {
    const STEER_UUID: &str = "00000000-0000-4000-8000-000000000001";
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claude")
        .join("live-2.1.296-steer-mid-turn.jsonl");
    let capture = std::fs::read_to_string(&fixture).expect("fixture readable");
    let lines: Vec<&str> = capture.lines().collect();
    let quiet = lines
        .iter()
        .position(|l| l.contains(r#""subtype":"task_notification""#))
        .expect("the tool's quiet stretch ends with its task_notification");
    let frame = |l: &str| serde_json::from_str::<serde_json::Value>(l).unwrap();
    let text_blocks: Vec<String> = lines
        .iter()
        .map(|l| frame(l))
        .filter(|f| f["type"] == "assistant")
        .flat_map(|f| {
            f["message"]["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|b| b["type"] == "text")
        .map(|b| b["text"].as_str().unwrap().to_owned())
        .collect();
    let [before_steer, reply] = &text_blocks[..] else {
        panic!("one text block on each side of the steer: {text_blocks:?}");
    };

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("before.jsonl"),
        lines[..quiet].join("\n") + "\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("after.jsonl"),
        lines[quiet..].join("\n") + "\n",
    )
    .unwrap();
    // Reads the prompt and the steer, then plays the capture with the
    // steer's real id in place of the captured one.
    let cli = dir.path().join("replay.sh");
    std::fs::write(
        &cli,
        format!(
            "#!/bin/sh\nread -r _first || exit 1\nread -r steer || exit 1\n\
             id=$(printf '%s\\n' \"$steer\" | sed 's/.*\"uuid\":\"\\([^\"]*\\)\".*/\\1/')\n\
             sed \"s/{STEER_UUID}/$id/g\" '{dir}/before.jsonl'\nsleep 6\n\
             sed \"s/{STEER_UUID}/$id/g\" '{dir}/after.jsonl'\n",
            dir = dir.path().display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let (controls, steer, _token) = controls("A");
    steer
        .send(SteerMessage {
            prompt: "Also say which directory you are in.".into(),
            message_id: None,
        })
        .await
        .unwrap();
    let stream = ClaudeHarness::new()
        .with_executable(&cli)
        .run(request("replay"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(
        Duration::from_secs(20),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time");

    let dones: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AgentEvent::Done { .. }).then_some(i))
        .collect();
    assert_eq!(
        dones,
        [events.len() - 1],
        "one Done, at the end: {events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some(result),
            ..
        }) if result == reply
    ));
    let steered = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("the replay confirms the steer");
    let text = |events: &[AgentEvent]| {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    // Every word, once, on the right side of the steer boundary.
    assert_eq!(&text(&events[..steered]), before_steer);
    assert_eq!(&text(&events[steered..]), reply);
}

/// Live smoke against the REAL claude CLI (2.1.x, must be installed + authed):
/// one trivial turn through the stdio permission channel, ending on the
/// result frame. `cargo test -p zeron-harness --test claude -- --ignored`.
#[tokio::test]
#[ignore = "spawns the real claude CLI; needs install + auth + network"]
async fn live_real_cli_single_turn() {
    let harness = ClaudeHarness::new();
    let mut req = request("Reply with exactly the word: pong");
    req.model = Some("haiku".into());
    req.cwd = std::env::temp_dir().display().to_string();
    req.auto_approve = false; // exercise --permission-prompt-tool stdio
    let (controls, _steer, _token) = controls("A");
    let mut stream = harness.run(req, controls).await.expect("run starts");
    // The session PARKS after the turn (steering mailbox still open, the CLI
    // waits for more stdin) — collect up to the first Done, not stream end.
    let events = tokio::time::timeout(Duration::from_secs(120), async {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            let done = matches!(ev, AgentEvent::Done { .. });
            events.push(ev);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("live turn finished in time");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. })),
        "{events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Slash-command discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn commands_come_from_the_initialize_control_request() {
    let h = harness();
    let commands = h.commands().await.expect("discovery succeeds");
    assert_eq!(
        commands.len(),
        2,
        "nameless entries are dropped: {commands:?}"
    );
    assert_eq!(commands[0].name, "review");
    assert_eq!(commands[0].description, "Review a pull request");
    assert_eq!(commands[0].input_hint.as_deref(), Some("[pr number]"));
    assert_eq!(commands[1].name, "compact");
    assert_eq!(commands[1].input_hint, None, "empty hint reads as None");

    // Cached: the second call reuses the first probe's result (the fake has
    // exited; a re-probe against a dead binary path would still work here,
    // but object identity of the cached list is the cheap assertion).
    let again = h.commands().await.expect("cache hit");
    assert_eq!(again, commands);
}

/// Live smoke against the real CLI: `cargo test -p zeron-harness --test
/// claude -- --ignored live_commands`. No model turn, no API cost.
#[tokio::test]
#[ignore]
async fn live_commands_discovery() {
    let h = ClaudeHarness::new();
    let commands = h.commands().await.expect("live discovery");
    assert!(!commands.is_empty());
    eprintln!("{} commands, first: {:?}", commands.len(), commands.first());
}

#[tokio::test]
async fn title_run_disables_tools_and_denies_unexpected_permissions() {
    let (controls, _steer, token) = controls("Yes");
    let mut stream = harness()
        .run_title(request("scenario:title"), controls)
        .await
        .unwrap();
    let events = tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
        events
    })
    .await
    .unwrap();
    token.cancel();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "Fix Login Flow")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn command_discovery_tracks_project_changes() {
    let h = harness();
    for name in ["project-a", "project-b"] {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join(".command-fixture"), name).unwrap();
        let commands = h
            .commands_for(&cwd.path().canonicalize().unwrap())
            .await
            .unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, name);
    }
}

#[tokio::test]
async fn shared_skill_colliding_with_builtin_keeps_file_delivery() {
    use zeron_proto::invocation::{Invocation, harness_prompt};
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    let directory = cwd.path().join(".agents/skills/compact");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("SKILL.md");
    std::fs::write(&path, "---\nname: compact\n---\nCompact JSON fixtures.").unwrap();

    let harness = harness();
    let commands = harness.commands_for(cwd.path()).await.unwrap();
    assert!(commands.iter().any(|command| command.name == "compact"));
    let skill = harness
        .skills(cwd.path())
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .find(|skill| skill.path == path.to_string_lossy())
        .unwrap();
    assert!(skill.command.is_none());
    let invocation = Invocation::Skill {
        name: skill.name,
        path: skill.path,
        command: skill.command,
    };
    assert_eq!(
        harness_prompt(
            &format!("{} data.json", invocation.link()),
            HarnessId::ClaudeCode
        ),
        format!("Use the skill {} data.json", invocation.prompt_text())
    );
}

#[tokio::test]
async fn claude_skills_follow_native_availability_and_dollar_selection_keeps_arguments() {
    use zeron_proto::{
        HarnessId,
        invocation::{Invocation, harness_prompt},
    };
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    for name in ["review", "disabled-plugin"] {
        let directory = cwd.path().join(".claude/skills").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test\n---\nInstructions"),
        )
        .unwrap();
    }
    let skills = harness().skills(cwd.path()).await.unwrap().unwrap();
    assert!(!skills.iter().any(|skill| skill.name == "disabled-plugin"));
    let skill = skills
        .into_iter()
        .find(|skill| skill.name == "review")
        .unwrap();
    assert_eq!(
        skill.command.as_ref().unwrap().harness,
        HarnessId::ClaudeCode
    );
    let invocation = Invocation::Skill {
        name: skill.name,
        path: skill.path,
        command: skill.command,
    };
    assert_eq!(
        harness_prompt(&format!("{} 123", invocation.link()), HarnessId::ClaudeCode),
        "/review 123"
    );
}

/// Rapid `now` steers: the CLI replays only the last one. The replay must
/// confirm every earlier steer too, and the run must still end.
#[tokio::test]
async fn a_replay_confirms_superseded_steers_and_the_turn_ends() {
    let (controls, steer, _token) = controls("A");
    for prompt in ["first steer", "second steer"] {
        steer
            .send(SteerMessage {
                prompt: prompt.into(),
                message_id: None,
            })
            .await
            .expect("steer queued");
    }
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        run_to_end(&harness(), request("scenario:superseded-steers"), controls),
    )
    .await
    .expect("run must end");
    let steered = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Steered { .. }))
        .count();
    assert_eq!(steered, 2, "{events:?}");
    let dones: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .collect();
    assert_eq!(dones.len(), 1, "{events:?}");
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "answered-both".into()
    }));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

/// A steer the CLI never replays must not hold the turn end forever.
#[tokio::test]
async fn an_unreplayed_steer_releases_the_turn_end() {
    let (controls, steer, _token) = controls("A");
    steer
        .send(SteerMessage {
            prompt: "absorbed steer".into(),
            message_id: None,
        })
        .await
        .expect("steer queued");
    let started = std::time::Instant::now();
    let mut stream = harness()
        .run(request("scenario:absorbed-steer"), controls)
        .await
        .expect("run starts");
    let mut steered = 0;
    let done = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while let Some(event) = stream.next().await {
            match event.expect("event") {
                AgentEvent::Steered { .. } => steered += 1,
                AgentEvent::Done { status, .. } => return status,
                _ => {}
            }
        }
        panic!("stream ended without Done");
    })
    .await
    .expect("turn end must be released");
    assert_eq!(done, DoneStatus::Completed);
    assert_eq!(steered, 1);
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));
}
