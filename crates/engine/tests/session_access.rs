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
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
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
