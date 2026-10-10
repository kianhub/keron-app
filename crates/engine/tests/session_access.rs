//! Full access is a per-session choice that the host enforces: a run takes
//! its access from the session's own config row, never from the request.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandEntry, SessionCommandPayload,
    SessionCommandStatus, SessionMessageEntry,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls, permissions};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, UserInputAnswer, UserInputQuestion,
};
use zeron_rpc::methods;

/// Records every request it is asked to run, and finishes at once.
struct Capture(Arc<Mutex<Vec<RunRequest>>>);

#[async_trait]
impl Harness for Capture {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Capture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.lock().unwrap().push(request);
        Ok(futures::stream::iter(vec![Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        })])
        .boxed())
    }
}

fn request(sandbox: SandboxLevel, auto_approve: bool) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: "go".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox,
        auto_approve,
        resume: None,
        attachments: vec![],
        worktree: None,
    }
}

async fn wait_for(requests: &Arc<Mutex<Vec<RunRequest>>>, count: usize) -> RunRequest {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(request) = requests.lock().unwrap().get(count - 1).cloned() {
                return request;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("run reached the harness")
}

fn message(id: &str, role: MessageRole, text: &str) -> SessionMessageEntry {
    SessionMessageEntry {
        duration_ms: None,
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: format!("{id}-text"),
            text: text.into(),
        }],
        created_at: 1,
        device_id: "device".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
    }
}

#[tokio::test]
async fn runs_take_their_access_from_the_session_not_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let sandbox = |chat: &str| {
        core.workspace
            .chat(chat)
            .unwrap()
            .and_then(|c| c.config)
            .map(|c| c.sandbox)
    };

    // A new session starts asking even when its creator asks for full
    // access (an agent's create_chat call, say).
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "s", "deviceId": core.device_id, "cwd": "/tmp",
                "config": { "harness": "mock", "sandbox": "danger-full-access" },
            }),
        )
        .await
        .unwrap();
    assert_eq!(sandbox("s"), Some(SandboxLevel::WorkspaceWrite));

    // A request can't widen a session that asks.
    core.sessions
        .dispatch(
            "s",
            HarnessId::Mock,
            request(SandboxLevel::DangerFullAccess, true),
            Some("m1".into()),
        )
        .await
        .unwrap();
    let ran = wait_for(&requests, 1).await;
    assert_eq!(ran.sandbox, SandboxLevel::WorkspaceWrite);
    assert!(!ran.auto_approve);

    // The session's own choice turns it on, for every run after.
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "setChatConfig", "chatId": "s",
                "config": { "harness": "mock", "sandbox": "danger-full-access" },
            }),
        )
        .await
        .unwrap();
    assert_eq!(sandbox("s"), Some(SandboxLevel::DangerFullAccess));
    core.sessions
        .dispatch(
            "s",
            HarnessId::Mock,
            request(SandboxLevel::WorkspaceWrite, false),
            Some("m2".into()),
        )
        .await
        .unwrap();
    let ran = wait_for(&requests, 2).await;
    assert_eq!(ran.sandbox, SandboxLevel::DangerFullAccess);
    assert!(ran.auto_approve);

    // A fork of a full-access session starts asking.
    let source = core.doc_host.open("s").unwrap();
    source
        .doc()
        .push_message(&message("u1", MessageRole::User, "hello"))
        .unwrap();
    source
        .doc()
        .push_message(&message("a1", MessageRole::Assistant, "hi"))
        .unwrap();
    let fork = client
        .call_as::<zeron_proto::Chat>(
            methods::FORK_SIDE_CHAT,
            serde_json::json!({ "chatId": "fork", "sourceChatId": "s" }),
        )
        .await
        .unwrap();
    assert_eq!(
        fork.config.map(|c| c.sandbox),
        Some(SandboxLevel::WorkspaceWrite)
    );
    assert_eq!(sandbox("fork"), Some(SandboxLevel::WorkspaceWrite));
}

#[tokio::test]
async fn sessions_this_devices_own_ui_starts_have_full_access_from_the_first_turn() {
    // With "New chats start with full access" on, the composer asks for it
    // in its createChat (and a fork, in its call). Only this device's own
    // UI is heard: the IPC port and the relay serve `rpc_service`.
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let host_ui = zeron_rpc::memory_client(core.host_ui_rpc_service());
    let other_client = zeron_rpc::memory_client(core.rpc_service());
    let sandbox = |chat: &str| {
        core.workspace
            .chat(chat)
            .unwrap()
            .and_then(|c| c.config)
            .map(|c| c.sandbox)
    };
    let create = |chat: &str, device: &str, full_access: bool| {
        serde_json::json!({
            "op": "createChat", "chatId": chat, "deviceId": device, "cwd": "/tmp",
            "config": { "harness": "mock", "sandbox": "workspace-write" },
            "fullAccess": full_access,
        })
    };
    // The composer's first send: createChat, then the Run it queues.
    let first_turn = async |client: &zeron_rpc::RpcClient, chat: &str, turn: usize| {
        let command = SessionCommandPayload::Run {
            request: request(SandboxLevel::WorkspaceWrite, false),
            message_id: format!("{chat}-m1"),
        };
        client
            .call(
                methods::QUEUE_COMMAND,
                serde_json::json!({ "chatId": chat, "command": command }),
            )
            .await
            .unwrap();
        wait_for(&requests, turn).await
    };

    host_ui
        .call(methods::MUTATE, create("mine", &core.device_id, true))
        .await
        .unwrap();
    assert_eq!(sandbox("mine"), Some(SandboxLevel::DangerFullAccess));
    let ran = first_turn(&host_ui, "mine", 1).await;
    assert_eq!(ran.sandbox, SandboxLevel::DangerFullAccess);
    assert!(ran.auto_approve);
    // The host's own choice counts: running it leaves the row as it was.
    assert_eq!(sandbox("mine"), Some(SandboxLevel::DangerFullAccess));

    // The same ask from any other client, with the setting off, or for the
    // voice orchestrator starts asking.
    let orchestrator = format!("{}1", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX);
    let asking = [
        (&other_client, "theirs", true),
        (&host_ui, "setting-off", false),
        (&host_ui, orchestrator.as_str(), true),
    ];
    for (turn, (client, chat, full_access)) in asking.into_iter().enumerate() {
        client
            .call(methods::MUTATE, create(chat, &core.device_id, full_access))
            .await
            .unwrap();
        assert_eq!(sandbox(chat), Some(SandboxLevel::WorkspaceWrite), "{chat}");
        let ran = first_turn(client, chat, turn + 2).await;
        assert_eq!(ran.sandbox, SandboxLevel::WorkspaceWrite, "{chat}");
        assert!(!ran.auto_approve, "{chat}");
    }
    // So does a session the UI starts on another device: that host decides.
    host_ui
        .call(methods::MUTATE, create("elsewhere", "other-device", true))
        .await
        .unwrap();
    assert_eq!(sandbox("elsewhere"), Some(SandboxLevel::WorkspaceWrite));

    // Sent before the UI resolved a harness, the createChat has no config:
    // the host writes one on the harness the run falls back to.
    host_ui
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "no-config", "deviceId": &core.device_id,
                "cwd": "/tmp", "fullAccess": true,
            }),
        )
        .await
        .unwrap();
    let config = core.workspace.chat("no-config").unwrap().unwrap().config;
    assert_eq!(
        config.map(|c| (c.harness, c.sandbox)),
        Some((HarnessId::Mock, SandboxLevel::DangerFullAccess))
    );
    let ran = first_turn(&host_ui, "no-config", 5).await;
    assert_eq!(ran.sandbox, SandboxLevel::DangerFullAccess);
    assert!(ran.auto_approve);

    // A fork follows the same rule as a new chat.
    let source = core.doc_host.open("theirs").unwrap();
    source
        .doc()
        .push_message(&message("u1", MessageRole::User, "hello"))
        .unwrap();
    source
        .doc()
        .push_message(&message("a1", MessageRole::Assistant, "hi"))
        .unwrap();
    for (client, fork, expected) in [
        (&host_ui, "fork-mine", SandboxLevel::DangerFullAccess),
        (&other_client, "fork-theirs", SandboxLevel::WorkspaceWrite),
    ] {
        let forked = client
            .call_as::<zeron_proto::Chat>(
                methods::FORK_SIDE_CHAT,
                serde_json::json!({ "chatId": fork, "sourceChatId": "theirs", "fullAccess": true }),
            )
            .await
            .unwrap();
        assert_eq!(forked.config.map(|c| c.sandbox), Some(expected), "{fork}");
        assert_eq!(sandbox(fork), Some(expected), "{fork}");
    }
}

#[tokio::test]
async fn a_queued_run_never_seeds_full_access_into_a_claimed_session() {
    // A Run can reach the host before its chat row does (claim on first
    // command); the host then stamps the row from the run. That stamp must
    // not carry a request's full access.
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let handle = core.doc_host.open("claimed").unwrap();
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-1".into(),
            payload: SessionCommandPayload::Run {
                request: request(SandboxLevel::DangerFullAccess, true),
                message_id: "m1".into(),
            },
            issued_by: "viewer-device".into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();
    let ran = wait_for(&requests, 1).await;
    assert_eq!(ran.sandbox, SandboxLevel::WorkspaceWrite);
    assert!(!ran.auto_approve);
    let row = core
        .workspace
        .chat("claimed")
        .unwrap()
        .expect("claimed row");
    assert_eq!(
        row.config.map(|c| c.sandbox),
        Some(SandboxLevel::WorkspaceWrite)
    );
}

#[tokio::test]
async fn a_session_minted_with_full_access_starts_asking() {
    // A phone or any other client writes its new session's row straight
    // into the registry, config included. Full access in that row is not
    // the session's own choice: the host runs it asking and says so in the
    // row, and only a later choice on the session turns full access on.
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let sandbox = |chat: &str| {
        core.workspace
            .chat(chat)
            .unwrap()
            .and_then(|c| c.config)
            .map(|c| c.sandbox)
    };
    // The same whole-row upsert a client's createSession writes.
    core.workspace
        .import_chat_row(&zeron_proto::Chat {
            id: "phone".into(),
            device_id: core.device_id.clone(),
            title: None,
            archived: false,
            cwd: Some("/tmp".into()),
            branch: None,
            checkout_id: None,
            source_context: None,
            config: Some(zeron_proto::ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            }),
            last_message_preview: None,
            last_message_at: None,
            created_at: chrono::Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: Some(2),
            parent_chat_id: None,
        })
        .unwrap();

    // Queued from the phone, with the request asking for full access too.
    let handle = core.doc_host.open("phone").unwrap();
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-1".into(),
            payload: SessionCommandPayload::Run {
                request: request(SandboxLevel::DangerFullAccess, true),
                message_id: "m1".into(),
            },
            issued_by: "phone-device".into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();
    let ran = wait_for(&requests, 1).await;
    assert_eq!(ran.sandbox, SandboxLevel::WorkspaceWrite);
    assert!(!ran.auto_approve);
    assert_eq!(sandbox("phone"), Some(SandboxLevel::WorkspaceWrite));

    // Turning it on from the session's own controls still works.
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "setChatConfig", "chatId": "phone",
                "config": { "harness": "mock", "sandbox": "danger-full-access" },
            }),
        )
        .await
        .unwrap();
    core.sessions
        .dispatch(
            "phone",
            HarnessId::Mock,
            request(SandboxLevel::WorkspaceWrite, false),
            Some("m2".into()),
        )
        .await
        .unwrap();
    let ran = wait_for(&requests, 2).await;
    assert_eq!(ran.sandbox, SandboxLevel::DangerFullAccess);
    assert!(ran.auto_approve);
    assert_eq!(sandbox("phone"), Some(SandboxLevel::DangerFullAccess));
}

#[tokio::test]
async fn only_the_voice_orchestrator_runs_its_zeron_tools_unasked() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    // The host only injects its server while it serves an IPC port.
    core.sessions.set_ipc_port(27699);
    let orchestrator = format!("{}1", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX);
    let config = zeron_proto::ChatConfig {
        harness: HarnessId::Mock,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    };
    for chat in [orchestrator.as_str(), "ordinary"] {
        core.workspace
            .create_chat(
                chat,
                None,
                Some(&core.device_id),
                Some(config.clone()),
                None,
            )
            .unwrap();
    }

    core.sessions
        .dispatch(
            &orchestrator,
            HarnessId::Mock,
            request(SandboxLevel::WorkspaceWrite, false),
            Some("m1".into()),
        )
        .await
        .unwrap();
    let ran = wait_for(&requests, 1).await;
    let mcp = ran
        .mcp
        .expect("the orchestrator's run carries the zeron server");
    assert_eq!(mcp.name, "zeron");
    assert!(
        mcp.approve_tools,
        "the orchestrator's own tools are pre-approved"
    );
    // Nothing else about the run is widened.
    assert_eq!(ran.sandbox, SandboxLevel::WorkspaceWrite);
    assert!(!ran.auto_approve);

    core.sessions
        .dispatch(
            "ordinary",
            HarnessId::Mock,
            request(SandboxLevel::WorkspaceWrite, false),
            Some("m2".into()),
        )
        .await
        .unwrap();
    let ran = wait_for(&requests, 2).await;
    let mcp = ran.mcp.expect("every run carries the zeron server");
    assert!(
        !mcp.approve_tools,
        "any other chat's zeron tools keep asking"
    );

    // A client can't send a pre-approved server: the flag never crosses the
    // wire, so a run queued with its own server asks for its tools.
    let sent: RunRequest = serde_json::from_value(serde_json::json!({
        "prompt": "go", "cwd": "/tmp", "sandbox": "workspace-write",
        "mcp": { "name": "zeron", "command": "zeron", "approveTools": true,
                 "approve_tools": true },
    }))
    .unwrap();
    assert!(!sent.mcp.unwrap().approve_tools);
}

/// Asks the user a question and then for one approval, the way the
/// harnesses that ask do (`permissions::approve`), and reports how the
/// approval went with the run's prompt. The turn ends once the question is
/// answered.
struct Asker(tokio::sync::mpsc::UnboundedSender<(String, bool)>);

#[async_trait]
impl Harness for Asker {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Asker"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let outcomes = self.0.clone();
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let RunControls {
                request_input,
                mut access,
                ..
            } = controls;
            let question = request_input(vec![UserInputQuestion {
                id: "pick".into(),
                header: "Choice".into(),
                question: "Pick one".into(),
                options: vec!["A".into(), "B".into()],
                multi_select: false,
                prefill: None,
                multiline: false,
            }]);
            let approval =
                permissions::approval_question("Approve command".into(), "Run `make`?".into());
            let approved = permissions::approve(&*request_input, &mut access, approval).await;
            let _ = outcomes.send((request.prompt, approved));
            let _ = question.await;
            let _ = events.send(Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }));
        });
        Ok(futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        )
        .boxed())
    }
}

/// The run's questions as they're asked: request id by header.
async fn questions(
    events: &mut tokio::sync::broadcast::Receiver<zeron_engine::JournaledEvent>,
) -> std::collections::HashMap<String, String> {
    let mut asked = std::collections::HashMap::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while asked.len() < 2 {
            if let AgentEvent::InputRequested {
                request_id,
                questions,
            } = events.recv().await.unwrap().event
            {
                asked.insert(questions[0].header.clone(), request_id);
            }
        }
    })
    .await
    .expect("both questions asked");
    asked
}

#[tokio::test]
async fn full_access_turned_on_mid_run_approves_only_that_sessions_waiting_approval() {
    let dir = tempfile::tempdir().unwrap();
    let (outcomes_tx, mut outcomes) = tokio::sync::mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Asker(outcomes_tx)));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());

    // Three sessions that ask, each with a question and an approval waiting.
    let mut asked = std::collections::HashMap::new();
    for chat in ["on", "other", "minted"] {
        client
            .call(
                methods::MUTATE,
                serde_json::json!({
                    "op": "createChat", "chatId": chat, "deviceId": core.device_id,
                    "cwd": "/tmp", "config": { "harness": "mock", "sandbox": "workspace-write" },
                }),
            )
            .await
            .unwrap();
        let (_, mut events) = core.sessions.subscribe(chat, 0).unwrap();
        core.sessions
            .dispatch(
                chat,
                HarnessId::Mock,
                RunRequest {
                    prompt: chat.into(),
                    ..request(SandboxLevel::WorkspaceWrite, false)
                },
                Some(format!("{chat}-m1")),
            )
            .await
            .unwrap();
        asked.insert(chat, (questions(&mut events).await, events));
    }

    // A whole-row write carrying full access isn't the session's choice: its
    // run keeps asking.
    core.workspace
        .import_chat_row(&zeron_proto::Chat {
            id: "minted".into(),
            device_id: core.device_id.clone(),
            title: None,
            archived: false,
            cwd: Some("/tmp".into()),
            branch: None,
            checkout_id: None,
            source_context: None,
            config: Some(zeron_proto::ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            }),
            last_message_preview: None,
            last_message_at: None,
            created_at: chrono::Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: Some(2),
            parent_chat_id: None,
        })
        .unwrap();

    // The owner turns full access on for one session mid-turn: its waiting
    // approval is approved, and the bridge resolves exactly that question,
    // as if the owner had answered it.
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "setChatConfig", "chatId": "on",
                "config": { "harness": "mock", "sandbox": "danger-full-access" },
            }),
        )
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
        .await
        .expect("the waiting approval settles");
    assert_eq!(outcome, Some(("on".to_owned(), true)));
    let (on, events) = asked.get_mut("on").unwrap();
    let resolved = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AgentEvent::InputResolved { request_id } = events.recv().await.unwrap().event {
                return request_id;
            }
        }
    })
    .await
    .expect("the approval's question resolves");
    assert_eq!(resolved, on["Approve command"]);
    assert!(
        !core
            .sessions
            .respond_input("on", &on["Approve command"], Vec::new())
            .unwrap(),
        "nothing left to answer"
    );
    // The real question still waits for the owner.
    assert!(
        core.sessions
            .respond_input(
                "on",
                &on["Choice"],
                vec![UserInputAnswer {
                    question_id: "pick".into(),
                    labels: vec!["B".into()],
                }],
            )
            .unwrap()
    );

    // No other session's approval moved: each still waits for its owner,
    // and a dismissal refuses it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(outcomes.try_recv().is_err());
    for chat in ["other", "minted"] {
        let (ids, _) = &asked[chat];
        assert!(
            core.sessions
                .respond_input(chat, &ids["Approve command"], Vec::new())
                .unwrap(),
            "{chat}'s approval still waits"
        );
        let outcome = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
            .await
            .unwrap();
        assert_eq!(outcome, Some((chat.to_owned(), false)));
    }
}

/// What a [`Parker`] runtime was handed: a fresh run, or a prompt steered
/// into the warm one.
#[derive(Debug, PartialEq)]
enum Turn {
    Run(String, SandboxLevel, bool),
    Steer(String),
}

/// A steerable runtime that stays warm between turns, the way Claude Code
/// and Codex do. A prompt starting with "hold" keeps its turn running until
/// the next steer joins it; every other turn ends at once.
struct Parker(tokio::sync::mpsc::UnboundedSender<Turn>);

#[async_trait]
impl Harness for Parker {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Parker"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let turns = self.0.clone();
        let _ = turns.send(Turn::Run(
            request.prompt.clone(),
            request.sandbox,
            request.auto_approve,
        ));
        let done = || {
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })
        };
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let RunControls {
                mut steering,
                interrupt,
                ..
            } = controls;
            if !request.prompt.starts_with("hold") {
                let _ = events.send(done());
            }
            loop {
                tokio::select! {
                    () = interrupt.cancelled() => return,
                    message = steering.recv() => {
                        let Some(message) = message else { return };
                        let _ = turns.send(Turn::Steer(message.prompt));
                        let _ = events.send(Ok(AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        }));
                        let _ = events.send(done());
                    }
                }
            }
        });
        Ok(futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        )
        .boxed())
    }
}

#[tokio::test]
async fn a_steer_between_turns_runs_with_the_access_the_session_has_now() {
    let dir = tempfile::tempdir().unwrap();
    let (turns_tx, mut turns) = tokio::sync::mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Parker(turns_tx)));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let set_access = |sandbox: &'static str| {
        client.call(
            methods::MUTATE,
            serde_json::json!({
                "op": "setChatConfig", "chatId": "s",
                "config": { "harness": "mock", "sandbox": sandbox },
            }),
        )
    };
    let handle = core.doc_host.open("s").unwrap();
    // A steer from any client: Steer on a queued row, an agent's
    // `send_message` with mode "steer".
    let steer = |id: &str| {
        handle
            .doc()
            .queue_command(&SessionCommandEntry {
                id: format!("cmd-{id}"),
                payload: SessionCommandPayload::Steer {
                    prompt: id.into(),
                    message_id: Some(format!("m-{id}")),
                },
                issued_by: "viewer-device".into(),
                issued_at: chrono::Utc::now().timestamp_millis(),
                based_on: None,
                expires_at: None,
                status: SessionCommandStatus::Pending,
                resolution: None,
            })
            .unwrap()
    };
    let mut next_turn = async || {
        tokio::time::timeout(Duration::from_secs(5), turns.recv())
            .await
            .expect("the prompt reached the agent")
            .unwrap()
    };
    let between_turns = async || {
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.sessions.turn_in_flight("s") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the turn ended")
    };

    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "s", "deviceId": core.device_id, "cwd": "/tmp",
                "config": { "harness": "mock", "sandbox": "workspace-write" },
            }),
        )
        .await
        .unwrap();
    set_access("danger-full-access").await.unwrap();
    core.sessions
        .dispatch(
            "s",
            HarnessId::Mock,
            RunRequest {
                prompt: "hold first".into(),
                ..request(SandboxLevel::WorkspaceWrite, false)
            },
            Some("m-first".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        next_turn().await,
        Turn::Run("hold first".into(), SandboxLevel::DangerFullAccess, true)
    );

    // Turned off mid-turn: a steer joins the running turn as it started.
    set_access("workspace-write").await.unwrap();
    steer("join");
    assert_eq!(next_turn().await, Turn::Steer("join".into()));
    between_turns().await;

    // The next turn asks: the warm runtime started with full access is
    // replaced, not handed the prompt.
    steer("next");
    assert_eq!(
        next_turn().await,
        Turn::Run("next".into(), SandboxLevel::WorkspaceWrite, false)
    );
    between_turns().await;

    // A runtime whose access still holds keeps taking prompts warm.
    steer("again");
    assert_eq!(next_turn().await, Turn::Steer("again".into()));
}
