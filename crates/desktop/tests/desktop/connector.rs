use crate::support::{define_agent, serve_websocket, serve_websocket_refusing};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::FakeAgent;
use acp_utils::websocket;
use aether_desktop::agents::{AgentProfile, Endpoint};
use aether_desktop::remotes::{Acp, InstanceState, RemoteSpec};
use aether_desktop::target::{AgentConnector, ConnectError, ProcessTransport, SessionTarget};
use aether_desktop::testing::{FAKE_AGENT_LOCK_FILE, launching_remote};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_remote::testing::FakeRemotes;
use client_core::session_model::WorkspaceAccess;
use serde_json::json;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::spawn_blocking;
use tokio_tungstenite::tungstenite::http::StatusCode;

#[tokio::test]
async fn a_stdio_agent_runs_in_the_threads_directory_and_dies_with_its_connection() {
    let cwd = tempfile::tempdir().unwrap();
    let agent =
        define_agent(cwd.path(), "fake", &json!({ "type": "stdio", "command": env!("CARGO_BIN_EXE_fake-acp-agent") }));

    let (session, runtime) = connector().connect(&target(cwd.path(), agent), None).await.unwrap().into_core();

    assert_eq!(session.session_response.session_id, SessionId::new("local-session"));
    let lock = File::open(cwd.path().join(FAKE_AGENT_LOCK_FILE)).expect("the agent started in the thread's directory");
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)), "the agent is running");

    drop(runtime);
    spawn_blocking(move || lock.lock()).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_websocket_agent_is_reached_at_its_url_with_its_headers_expanded() {
    let workspace = tempfile::tempdir().unwrap();
    let server = serve_websocket(FakeAgent::new("remote-session")).await;
    let agent = define_agent(
        workspace.path(),
        "remote",
        &json!({ "type": "websocket", "url": server.url, "headers": { "X-Workspace": "${WORKSPACE}" } }),
    );

    let (session, _) = connector().connect(&target(workspace.path(), agent), None).await.unwrap().into_core();

    assert_eq!(session.session_response.session_id, SessionId::new("remote-session"));
    let headers = server.headers.await.unwrap();
    assert_eq!(headers["x-workspace"], workspace.path().to_str().unwrap());
}

#[tokio::test]
async fn a_websocket_agent_is_reached_wherever_its_endpoint_command_says() {
    let workspace = tempfile::tempdir().unwrap();
    let server = serve_websocket(FakeAgent::new("remote-session")).await;
    let endpoint = json!({ "url": server.url, "headers": { "Authorization": "Bearer minted" } }).to_string();
    let agent = define_agent(
        workspace.path(),
        "remote",
        &json!({ "type": "websocket", "endpoint": { "command": "printf", "args": ["%s", endpoint] } }),
    );

    let (session, _) = connector().connect(&target(workspace.path(), agent), None).await.unwrap().into_core();

    assert_eq!(session.session_response.session_id, SessionId::new("remote-session"));
    assert_eq!(server.headers.await.unwrap()["authorization"], "Bearer minted");
}

#[tokio::test]
async fn an_aether_remote_server_opens_its_live_session_in_its_own_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let remote = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: Some("live".into()) };
    let server = serve_websocket(FakeAgent::default().remote_server(&remote)).await;
    let agent = define_agent(workspace.path(), "vm", &json!({ "type": "websocket", "url": server.url }));

    let (session, _) = connector().connect(&target(workspace.path(), agent), None).await.unwrap().into_core();

    assert_eq!(session.session_response.session_id, SessionId::new("live"));
    assert_eq!(session.working_dir, PathBuf::from("/vm/workspace"));
    assert_eq!(session.workspace_access, WorkspaceAccess::Remote);
}

#[tokio::test]
async fn a_thread_on_a_remote_launches_an_instance_and_reaches_its_agent_where_the_command_says() {
    let remotes = FakeRemotes::default();
    let url = serve_websocket_refusing(FakeAgent::new("vm-session"), 1, StatusCode::BAD_GATEWAY).await.url;
    let endpoint = json!({ "url": url, "headers": { "X-aws-proxy-auth": "token" } }).to_string();
    let script = "test \"$1\" = instance-0 && printf '%s' \"$2\"";
    let acp = ["sh", "-c", script, "sh", "${ID}", &endpoint].map(str::to_owned).to_vec();
    let remote = RemoteSpec {
        dir: None,
        acp: Some(Acp { cwd: Some("/vm/patchwave".to_owned()), ..Acp::websocket(acp) }),
        ..launching_remote("patchwave", "/patchwave")
    };
    let connector = AgentConnector::new(Arc::new(remotes.clone()), Arc::new(ProcessTransport));
    let mut target = SessionTarget::remote(&remote, None);

    let launched = connector.launch(&target).await.unwrap();
    target = SessionTarget::Remote { remote, instance: launched.clone() };
    let (session, _) = connector.connect(&target, None).await.unwrap().into_core();

    assert_eq!(launched.as_deref(), Some("instance-0"));
    assert_eq!(remotes.state_of("patchwave", "instance-0"), Some(InstanceState::Running));
    assert_eq!(session.session_response.session_id, SessionId::new("vm-session"));
    assert_eq!(session.working_dir, PathBuf::from("/vm/patchwave"), "the agent works in the remote's `acp.cwd`");
    assert_eq!(connector.launch(&target).await.unwrap(), None, "a launched instance is reused");
    assert_eq!(connector.diff_endpoint(&target).await.unwrap(), None, "the agent diffs its own working tree");
}

#[tokio::test]
async fn a_static_remotes_agent_runs_over_stdio_in_the_remotes_directory_without_launching() {
    let dir = tempfile::tempdir().unwrap();
    let remotes = FakeRemotes::default();
    let remote = RemoteSpec {
        dir: Some(dir.path().to_owned()),
        acp: Some(Acp {
            cwd: Some("/home/me/code".to_owned()),
            ..Acp::stdio(vec![env!("CARGO_BIN_EXE_fake-acp-agent").to_owned()])
        }),
        ..RemoteSpec::named("devbox")
    };
    let connector = AgentConnector::new(Arc::new(remotes.clone()), Arc::new(ProcessTransport));
    let target = SessionTarget::remote(&remote, None);

    let launched = connector.launch(&target).await.unwrap();
    let (session, _runtime) = connector.connect(&target, None).await.unwrap().into_core();

    assert_eq!(launched, None, "a static remote's one instance is always running");
    assert_eq!(target.instance(), Some("devbox"));
    assert_eq!(remotes.launches(), 0);
    assert_eq!(session.session_response.session_id, SessionId::new("local-session"));
    assert!(dir.path().join(FAKE_AGENT_LOCK_FILE).exists(), "the agent started in the remote's directory");
}

#[test]
fn a_thread_on_a_remote_has_no_local_directory_to_mention_files_from() {
    let remote = launching_remote("patchwave", "/patchwave");

    let target = SessionTarget::remote(&remote, Some("instance-0".to_owned()));

    assert_eq!(target.cwd(), None);
}

#[tokio::test]
async fn a_remotes_agent_without_a_cwd_must_pick_its_own() {
    let server = serve_websocket(FakeAgent::new("vm-session")).await;
    let endpoint = json!({ "url": server.url }).to_string();
    let remote = RemoteSpec {
        acp: Some(Acp::websocket(["printf", "%s", &endpoint].map(str::to_owned).to_vec())),
        ..RemoteSpec::named("devbox")
    };

    let error = connector().connect(&SessionTarget::remote(&remote, None), None).await.err();

    assert!(matches!(error, Some(ConnectError::NoCwd { ref remote }) if remote == "devbox"), "{error:?}");
}

#[tokio::test]
async fn an_aether_server_on_a_remote_without_a_cwd_works_in_its_own_workspace() {
    let info = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: Some("live".into()) };
    let server = serve_websocket(FakeAgent::default().remote_server(&info)).await;
    let endpoint = json!({ "url": server.url }).to_string();
    let remote = RemoteSpec {
        acp: Some(Acp::websocket(["printf", "%s", &endpoint].map(str::to_owned).to_vec())),
        ..RemoteSpec::named("devbox")
    };

    let (session, _) = connector().connect(&SessionTarget::remote(&remote, None), None).await.unwrap().into_core();

    assert_eq!(session.working_dir, PathBuf::from("/vm/workspace"));
    assert_eq!(session.workspace_access, WorkspaceAccess::Remote);
}

#[tokio::test]
async fn a_thread_on_a_remote_cannot_connect_before_its_instance_is_launched() {
    let remote = launching_remote("patchwave", "/patchwave");

    let error = AgentConnector::new(Arc::new(FakeRemotes::default()), Arc::new(ProcessTransport))
        .connect(&SessionTarget::remote(&remote, None), None)
        .await
        .err()
        .unwrap();

    assert!(matches!(error, ConnectError::NotLaunched), "{error}");
}

#[tokio::test]
async fn an_agents_diff_endpoint_is_its_url_with_its_headers_expanded() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(
        workspace.path(),
        "claude",
        &json!({
            "type": "stdio",
            "command": "claude-code-acp",
            "diff": { "url": "ws://vm/diff", "headers": { "X-Workspace": "${WORKSPACE}" } },
        }),
    );

    let endpoint = connector().diff_endpoint(&target(workspace.path(), agent)).await.unwrap();

    let headers = [("X-Workspace".to_owned(), workspace.path().to_string_lossy().into_owned())].into();
    assert_eq!(endpoint, Some(Endpoint { url: "ws://vm/diff".to_owned(), headers }));
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

    let endpoint = connector().diff_endpoint(&target(workspace.path(), agent)).await.unwrap();

    let headers = [("Authorization".to_owned(), "Bearer minted".to_owned())].into();
    assert_eq!(endpoint, Some(Endpoint { url: "ws://vm:7331/ws".to_owned(), headers }));
}

#[tokio::test]
async fn an_agent_without_a_diff_endpoint_has_none() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = define_agent(workspace.path(), "claude", &json!({ "type": "stdio", "command": "claude-code-acp" }));

    assert_eq!(connector().diff_endpoint(&target(workspace.path(), agent)).await.unwrap(), None);
}

#[tokio::test]
async fn an_agent_that_cannot_be_reached_says_why() {
    let refusing = |status| serve_websocket_refusing(FakeAgent::default(), usize::MAX, status);
    let (down, occupied) = (refusing(StatusCode::BAD_GATEWAY).await.url, refusing(StatusCode::CONFLICT).await.url);
    let failing_endpoint =
        json!({ "command": "sh", "args": ["-c", "echo starting >&2; echo 'no credentials' >&2; exit 3"] });
    let cases: [(serde_json::Value, IsExpected); 6] = [
        (json!({ "type": "stdio", "command": "no-such-agent-binary" }), |error| {
            matches!(error, ConnectError::Spawn { .. })
        }),
        (json!({ "type": "websocket", "url": down }), |error| {
            matches!(error, ConnectError::WebSocket(websocket::ConnectError::Handshake(_)))
        }),
        (json!({ "type": "websocket", "url": occupied }), |error| {
            matches!(error, ConnectError::WebSocket(websocket::ConnectError::Occupied))
        }),
        (json!({ "type": "websocket", "url": "ws://${AETHER_DESKTOP_TEST_UNSET_HOST}:8765" }), |error| {
            matches!(error, ConnectError::Variable(_))
        }),
        (
            json!({ "type": "websocket", "endpoint": failing_endpoint }),
            |error| matches!(error, ConnectError::CommandFailed { stderr, .. } if stderr == "no credentials"),
        ),
        (json!({ "type": "websocket", "endpoint": { "command": "echo", "args": ["booting"] } }), |error| {
            matches!(error, ConnectError::CommandOutput { .. })
        }),
    ];

    for (config, expected) in cases {
        let workspace = tempfile::tempdir().unwrap();
        let target = target(workspace.path(), define_agent(workspace.path(), "agent", &config));
        let error = connector().connect(&target, None).await.err().unwrap();
        assert!(expected(&error), "{config}: {error}");
    }
}

type IsExpected = fn(&ConnectError) -> bool;

fn target(cwd: &Path, agent: AgentProfile) -> SessionTarget {
    SessionTarget::agent(cwd.into(), agent)
}

fn connector() -> AgentConnector {
    AgentConnector::new(Arc::new(FakeRemotes::default()), Arc::new(ProcessTransport))
}
