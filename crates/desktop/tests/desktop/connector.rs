use crate::support::{define_agent, refuse_websocket, serve_websocket, serve_websocket_once_started};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::FakeAgent;
use aether_desktop::agents::AgentProfile;
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
async fn a_stdio_agent_command_that_does_not_exist_fails_to_start() {
    let cwd = tempfile::tempdir().unwrap();
    let agent = define_agent(cwd.path(), "missing", &json!({ "type": "stdio", "command": "no-such-agent-binary" }));

    let error = AgentConnector.connect(&target(cwd.path(), agent), ConnectMode::NewSession).await.err().unwrap();

    assert!(matches!(error, ConnectError::Spawn { .. }), "{error}");
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
async fn a_launched_agent_is_reached_and_terminated_through_its_instance() {
    let workspace = tempfile::tempdir().unwrap();
    let server = serve_websocket(new_session("vm-session")).await;
    let endpoint = json!({ "url": server.url }).to_string();
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
    let mut target = target(workspace.path(), agent);

    target.instance = AgentConnector.launch(&target).await.unwrap();
    let session = AgentConnector.connect(&target, ConnectMode::NewSession).await.unwrap();
    AgentConnector.terminate(&target).await.unwrap();

    assert_eq!(target.instance, Some(InstanceId::new("vm-1")));
    assert_eq!(session.response.session_id, SessionId::new("vm-session"));
    assert!(workspace.path().join("terminated-vm-1").exists(), "terminate ran in the workspace, for the instance");
}

#[tokio::test]
async fn a_launched_agent_is_reached_once_the_server_behind_its_endpoint_has_started() {
    let workspace = tempfile::tempdir().unwrap();
    let url = serve_websocket_once_started(new_session("vm-session"), 1).await;
    let agent = define_agent(
        workspace.path(),
        "vm",
        &json!({
            "type": "websocket",
            "launch": { "command": "printf", "args": ["%s", r#"{"id": "vm-1"}"#] },
            "endpoint": { "command": "printf", "args": ["%s", json!({ "url": url }).to_string()] },
        }),
    );
    let mut target = target(workspace.path(), agent);
    target.instance = AgentConnector.launch(&target).await.unwrap();

    let session = AgentConnector.connect(&target, ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id, SessionId::new("vm-session"));
}

#[tokio::test]
async fn an_agent_that_is_not_launched_fails_at_once_while_its_server_is_down() {
    let workspace = tempfile::tempdir().unwrap();
    let url = refuse_websocket(StatusCode::BAD_GATEWAY).await;
    let agent = define_agent(workspace.path(), "remote", &json!({ "type": "websocket", "url": url }));

    let error = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.err().unwrap();

    assert!(matches!(error, ConnectError::WebSocket(_)), "{error}");
}

#[tokio::test]
async fn an_agent_that_launches_nothing_has_no_instance_to_terminate() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(workspace.path(), "remote", &json!({ "type": "websocket", "url": "ws://127.0.0.1:1" }));
    let target = target(workspace.path(), agent);

    assert_eq!(AgentConnector.launch(&target).await.unwrap(), None);
    AgentConnector.terminate(&target).await.unwrap();
}

#[tokio::test]
async fn a_failed_launch_reports_the_last_line_its_command_printed_on_stderr() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(
        workspace.path(),
        "vm",
        &json!({
            "type": "websocket",
            "launch": { "command": "sh", "args": ["-c", "echo starting >&2; echo 'no credentials' >&2; exit 3"] },
            "endpoint": { "command": "true" },
        }),
    );

    let error = AgentConnector.launch(&target(workspace.path(), agent)).await.unwrap_err();

    assert!(matches!(&error, ConnectError::CommandFailed { stderr, .. } if stderr == "no credentials"), "{error}");
}

#[tokio::test]
async fn a_launch_command_that_prints_no_instance_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(
        workspace.path(),
        "vm",
        &json!({ "type": "websocket", "launch": { "command": "echo", "args": ["booting"] }, "endpoint": { "command": "true" } }),
    );

    let error = AgentConnector.launch(&target(workspace.path(), agent)).await.unwrap_err();

    assert!(matches!(error, ConnectError::CommandOutput { .. }), "{error}");
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
async fn a_websocket_agent_with_a_client_attached_already_is_reported_occupied() {
    let workspace = tempfile::tempdir().unwrap();
    let url = refuse_websocket(StatusCode::CONFLICT).await;
    let agent = define_agent(workspace.path(), "vm", &json!({ "type": "websocket", "url": url }));

    let error = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.err().unwrap();

    assert!(matches!(error, ConnectError::Occupied), "{error}");
}

#[tokio::test]
async fn an_unset_variable_fails_the_connection_before_reaching_the_agent() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(
        workspace.path(),
        "remote",
        &json!({ "type": "websocket", "url": "ws://${AETHER_DESKTOP_TEST_UNSET_HOST}:8765" }),
    );

    let error = AgentConnector.connect(&target(workspace.path(), agent), ConnectMode::NewSession).await.err().unwrap();

    assert!(matches!(error, ConnectError::Variable(_)), "{error}");
}

fn target(cwd: &Path, agent: AgentProfile) -> SessionTarget {
    SessionTarget::new(cwd.into(), agent)
}

fn new_session(session_id: &str) -> FakeAgent {
    FakeAgent::default().new_session_response(NewSessionResponse::new(session_id))
}
