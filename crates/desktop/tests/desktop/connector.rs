use crate::support::{define_agent, refuse_websocket, serve_websocket, serve_websocket_once_started};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::FakeAgent;
use aether_desktop::agents::{AgentProfile, Endpoint};
use aether_desktop::target::{AgentConnector, ConnectError, ConnectMode, Connector, InstanceId, SessionTarget};
use aether_desktop::testing::FAKE_AGENT_LOCK_FILE;
use agent_client_protocol::schema::v2::{NewSessionResponse, SessionId};
use client_core::workspace_status::WorkspaceAccess;
use serde_json::json;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use tokio::task::spawn_blocking;
use tokio_tungstenite::tungstenite::http::StatusCode;

#[tokio::test]
async fn a_stdio_agent_runs_in_the_threads_directory_and_dies_with_its_connection() {
    let cwd = tempfile::tempdir().unwrap();
    let agent =
        define_agent(cwd.path(), "fake", &json!({ "type": "stdio", "command": env!("CARGO_BIN_EXE_fake-acp-agent") }));

    let session = AgentConnector.connect(&target(cwd.path(), agent), ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id.0.as_ref(), "local-session");
    let lock = File::open(cwd.path().join(FAKE_AGENT_LOCK_FILE)).expect("the agent started in the thread's directory");
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)), "the agent is running");

    drop(session);
    // Blocks until the agent's process is gone and its lock with it.
    spawn_blocking(move || lock.lock()).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_websocket_agent_is_reached_at_its_url_with_its_headers_expanded() {
    let workspace = tempfile::tempdir().unwrap();
    let server = serve_websocket(new_session("remote-session")).await;
    let agent = define_agent(
        workspace.path(),
        "remote",
        &json!({ "type": "websocket", "url": server.url, "headers": { "X-Workspace": "${WORKSPACE}" } }),
    );

    let session = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id, SessionId::new("remote-session"));
    let headers = server.headers.await.unwrap();
    assert_eq!(headers["x-workspace"], workspace.path().to_str().unwrap());
}

#[tokio::test]
async fn a_websocket_agent_is_reached_wherever_its_endpoint_command_says() {
    let workspace = tempfile::tempdir().unwrap();
    let server = serve_websocket(new_session("remote-session")).await;
    let endpoint = json!({ "url": server.url, "headers": { "Authorization": "Bearer minted" } }).to_string();
    let agent = define_agent(
        workspace.path(),
        "remote",
        &json!({ "type": "websocket", "endpoint": { "command": "printf", "args": ["%s", endpoint] } }),
    );

    let session = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id, SessionId::new("remote-session"));
    assert_eq!(server.headers.await.unwrap()["authorization"], "Bearer minted");
}

#[tokio::test]
async fn an_aether_remote_server_opens_its_live_session_in_its_own_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let remote = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: Some("live".into()) };
    let server = serve_websocket(FakeAgent::default().remote_server(&remote)).await;
    let agent = define_agent(workspace.path(), "vm", &json!({ "type": "websocket", "url": server.url }));

    let session = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id, SessionId::new("live"));
    assert_eq!(session.working_dir, PathBuf::from("/vm/workspace"));
    assert_eq!(session.workspace_access, WorkspaceAccess::Remote);
}

#[tokio::test]
async fn a_launched_agent_is_reached_once_its_server_starts_and_terminated_through_its_instance() {
    let workspace = tempfile::tempdir().unwrap();
    let url = serve_websocket_once_started(new_session("vm-session"), 1).await;
    let endpoint = json!({ "url": url }).to_string();
    let agent = define_agent(
        workspace.path(),
        "vm",
        &json!({
            "type": "websocket",
            "launch": { "command": "printf", "args": ["%s", r#"{"id": "vm-1"}"#] },
            "endpoint": { "command": "sh", "args": ["-c", "test \"$1\" = vm-1 && printf '%s' \"$2\"", "sh", "${INSTANCE_ID}", endpoint] },
            "terminate": { "command": "touch", "args": ["terminated-${INSTANCE_ID}"] },
        }),
    );
    let mut vm = target(workspace.path(), agent);

    vm.instance = AgentConnector.launch(&vm).await.unwrap();
    let session = AgentConnector.connect(&vm, ConnectMode::NewSession).await.unwrap();
    AgentConnector.terminate(&vm).await.unwrap();

    assert_eq!(vm.instance, Some(InstanceId::new("vm-1")));
    assert_eq!(session.response.session_id, SessionId::new("vm-session"));
    assert!(workspace.path().join("terminated-vm-1").exists(), "terminate ran in the workspace, for the instance");
    let plain = define_agent(workspace.path(), "remote", &json!({ "type": "websocket", "url": "ws://127.0.0.1:1" }));
    assert_eq!(AgentConnector.launch(&target(workspace.path(), plain)).await.unwrap(), None, "nothing to launch");
}

#[tokio::test]
async fn an_agents_diff_endpoint_is_its_url_with_its_headers_expanded_for_its_instance() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(
        workspace.path(),
        "claude",
        &json!({
            "type": "stdio",
            "command": "claude-code-acp",
            "diff": { "url": "ws://vm/${INSTANCE_ID}", "headers": { "X-Workspace": "${WORKSPACE}" } },
        }),
    );
    let mut vm = target(workspace.path(), agent);
    vm.instance = Some(InstanceId::new("vm-1"));

    let endpoint = AgentConnector.diff_endpoint(&vm).await.unwrap();

    let headers = [("X-Workspace".to_owned(), workspace.path().to_string_lossy().into_owned())].into();
    assert_eq!(endpoint, Some(Endpoint { url: "ws://vm/vm-1".to_owned(), headers }));
}

#[tokio::test]
async fn an_agents_diff_endpoint_is_wherever_its_endpoint_command_says() {
    let workspace = tempfile::tempdir().unwrap();
    let printed = json!({ "url": "ws://vm:7331/ws", "headers": { "Authorization": "Bearer minted" } }).to_string();
    let agent = define_agent(
        workspace.path(),
        "claude",
        &json!({
            "type": "stdio",
            "command": "claude-code-acp",
            "diff": { "endpoint": { "command": "printf", "args": ["%s", printed] } },
        }),
    );

    let endpoint = AgentConnector.diff_endpoint(&target(workspace.path(), agent)).await.unwrap();

    let headers = [("Authorization".to_owned(), "Bearer minted".to_owned())].into();
    assert_eq!(endpoint, Some(Endpoint { url: "ws://vm:7331/ws".to_owned(), headers }));
}

#[tokio::test]
async fn an_agent_without_a_diff_endpoint_has_none() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(workspace.path(), "claude", &json!({ "type": "stdio", "command": "claude-code-acp" }));

    assert_eq!(AgentConnector.diff_endpoint(&target(workspace.path(), agent)).await.unwrap(), None);
}

#[tokio::test]
async fn an_agent_that_cannot_be_launched_or_reached_says_why() {
    let down = refuse_websocket(StatusCode::BAD_GATEWAY).await;
    let occupied = refuse_websocket(StatusCode::CONFLICT).await;
    let failing_launch =
        json!({ "command": "sh", "args": ["-c", "echo starting >&2; echo 'no credentials' >&2; exit 3"] });
    let cases: [(serde_json::Value, IsExpected); 6] = [
        (json!({ "type": "stdio", "command": "no-such-agent-binary" }), |error| {
            matches!(error, ConnectError::Spawn { .. })
        }),
        (json!({ "type": "websocket", "url": down }), |error| matches!(error, ConnectError::WebSocket(_))),
        (json!({ "type": "websocket", "url": occupied }), |error| matches!(error, ConnectError::Occupied)),
        (json!({ "type": "websocket", "url": "ws://${AETHER_DESKTOP_TEST_UNSET_HOST}:8765" }), |error| {
            matches!(error, ConnectError::Variable(_))
        }),
        (
            json!({ "type": "websocket", "launch": failing_launch, "endpoint": { "command": "true" } }),
            |error| matches!(error, ConnectError::CommandFailed { stderr, .. } if stderr == "no credentials"),
        ),
        (
            json!({ "type": "websocket", "launch": { "command": "echo", "args": ["booting"] }, "endpoint": { "command": "true" } }),
            |error| matches!(error, ConnectError::CommandOutput { .. }),
        ),
    ];

    for (config, expected) in cases {
        let workspace = tempfile::tempdir().unwrap();
        let target = target(workspace.path(), define_agent(workspace.path(), "agent", &config));
        let error = match AgentConnector.launch(&target).await {
            Ok(_) => AgentConnector.connect(&target, ConnectMode::NewSession).await.err().unwrap(),
            Err(error) => error,
        };
        assert!(expected(&error), "{config}: {error}");
    }
}

/// Whether a connection failed the way a case expects.
type IsExpected = fn(&ConnectError) -> bool;

fn target(cwd: &Path, agent: AgentProfile) -> SessionTarget {
    SessionTarget::new(cwd.into(), agent)
}

fn new_session(session_id: &str) -> FakeAgent {
    FakeAgent::default().new_session_response(NewSessionResponse::new(session_id))
}
