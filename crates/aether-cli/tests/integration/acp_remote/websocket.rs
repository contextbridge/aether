use super::{assert_no_ended_turn, assert_stopped_turn, start_paused_turn};
use acp_utils::client::{AcpClient, AcpEvent, connect_acp_client};
use acp_utils::testing::initialize_request;
use acp_utils::websocket::WebSocketTransport;
use aether_cli::acp::server::{ServerArgs, ServerRunError, run_server};
use aether_cli::acp::testing::{AcpTestHarness, AcpWebSocketTestServer};
use aether_core::events::{AgentEvent, MessageEvent, TurnEvent, TurnOutcome};
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::{
    AbsolutePath, CancelSessionNotification, CloseSessionRequest, ContentBlock, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SessionId, SessionUpdate, StateUpdate, StopReason,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::LocalSet;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error, Message, client::IntoClientRequest, http::StatusCode};

#[derive(Debug, Parser)]
struct ServerCli {
    #[command(flatten)]
    args: ServerArgs,
}

#[test]
fn server_args_defaults_and_overrides() {
    let defaults = ServerCli::parse_from(["server"]).args;
    assert_eq!(defaults.listen, "127.0.0.1:8765".parse().unwrap());
    assert_eq!(defaults.cwd, PathBuf::from("."));
    let args = ServerCli::parse_from([
        "server",
        "--listen",
        "0.0.0.0:9000",
        "-C",
        "/workspace",
        "--agent",
        "Build",
        "--log-dir",
        "/logs",
    ])
    .args;
    assert_eq!(args.listen, "0.0.0.0:9000".parse().unwrap());
    assert_eq!(args.cwd, PathBuf::from("/workspace"));
    assert_eq!(args.acp.agent.as_deref(), Some("Build"));
    assert_eq!(args.acp.log_dir, Some(PathBuf::from("/logs")));
    assert_eq!(ServerCli::parse_from(["server", "--cwd", "/other"]).args.cwd, PathBuf::from("/other"));
}

#[test]
fn server_args_reuse_acp_validation() {
    for arguments in [
        vec!["server", "--agent", "Build", "--model", "anthropic:claude-sonnet-4-5"],
        vec!["server", "--settings-json", "{}", "--settings-file", "settings.json"],
        vec!["server", "--options-json", "{}", "--agent", "Build"],
    ] {
        assert_eq!(ServerCli::try_parse_from(arguments).unwrap_err().kind(), clap::error::ErrorKind::ArgumentConflict);
    }
    assert!(ServerCli::try_parse_from(["server", "--listen", "not-an-address"]).is_err());
}

#[tokio::test]
async fn server_rejects_missing_and_non_directory_workspaces() {
    let directory = tempfile::tempdir().unwrap();
    let file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    for path in [directory.path().join("missing"), file.path().to_path_buf()] {
        let args = ServerCli::parse_from(["server", "--cwd", path.to_str().unwrap()]).args;
        assert!(matches!(run_server(args).await, Err(ServerRunError::Workspace { .. })));
    }
}

#[tokio::test]
async fn in_memory_client_and_multiple_listeners_share_one_slot() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let first = harness.listen_websocket().await;
            let second = harness.listen_websocket().await;
            assert_occupied(&first).await;
            assert_occupied(&second).await;
            harness.disconnect().await;
            let (mut owner, _) = connect_async(first.url()).await.unwrap();
            assert_occupied(&second).await;
            let mut detached = first.subscribe();
            let seen = *detached.borrow();
            owner.close(None).await.unwrap();
            assert!(matches!(owner.next().await, Some(Ok(Message::Close(_)))));
            detached.wait_for(|g| *g > seen).await.unwrap();
            let (owner, _) = connect_async(second.url()).await.unwrap();
            assert_occupied(&first).await;
            drop(owner);
            first.shutdown().await;
            second.shutdown().await;
            harness.shutdown().await;
        }))
        .await;
}

#[tokio::test]
async fn second_websocket_is_rejected_without_detaching_owner() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut owner = connect(&server, &id).await;
            expect_running(&mut owner).await;
            assert_occupied(&server).await;
            assert!(owner.handle.prompt(PromptRequest::new(id.clone(), vec!["still busy".into()])).await.is_err());
            release.notify_one();
            expect_completed(&mut owner, &id, Some(StopReason::EndTurn)).await;
            assert_persisted_response(&harness, &id);
            server.shutdown().await;
            expect_closed(&mut owner).await;
            harness.shutdown().await;
            assert_eq!(harness.live_runtime_count(), 0);
        }))
        .await;
}

#[tokio::test]
async fn occupied_server_drops_non_websocket_request_without_detaching_owner() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let server = harness.serve_websocket().await;
            let (owner, _) = connect_async(server.url()).await.unwrap();
            let mut invalid = TcpStream::connect(server.address).await.unwrap();
            invalid.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            let mut response = Vec::new();
            invalid.read_to_end(&mut response).await.unwrap();
            assert!(response.is_empty());
            assert_occupied(&server).await;
            drop(owner);
            server.shutdown().await;
            harness.shutdown().await;
        }))
        .await;
}

#[tokio::test]
async fn broken_websocket_detaches_and_reattaches_original_turn() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut first = connect(&server, &id).await;
            expect_running(&mut first).await;
            let mut detached = server.subscribe();
            let seen = *detached.borrow();
            first.handle.disconnect().await;
            detached.wait_for(|g| *g > seen).await.unwrap();
            assert_no_ended_turn(&harness, &id);
            let mut second = connect(&server, &id).await;
            expect_running(&mut second).await;
            release.notify_one();
            expect_completed(&mut second, &id, Some(StopReason::EndTurn)).await;
            assert_persisted_response(&harness, &id);
            let seen = *detached.borrow();
            second.handle.disconnect().await;
            detached.wait_for(|g| *g > seen).await.unwrap();
            let mut third = connect(&server, &id).await;
            expect_completed(&mut third, &id, None).await;
            assert_persisted_response(&harness, &id);
            server.shutdown().await;
            harness.shutdown().await;
        }))
        .await;
}

#[tokio::test]
async fn explicit_remote_cancel_and_close_remain_destructive() {
    LocalSet::new().run_until(Box::pin(async {
        let mut harness = AcpTestHarness::start().await;
        let (id, _release) = start_paused_turn(&mut harness).await;
        let server = harness.serve_websocket().await;
        let mut client = connect(&server, &id).await;
        expect_running(&mut client).await;
        client.handle.cancel(CancelSessionNotification::new(id.clone())).unwrap();
        loop {
            match client.event_rx.recv().await.unwrap() {
                AcpEvent::SessionUpdate(update) if matches!(update.update,
                    SessionUpdate::StateUpdate(StateUpdate::Idle(ref idle)) if idle.stop_reason == Some(StopReason::Cancelled)) => break,
                AcpEvent::ConnectionClosed => panic!("cancel must not disconnect the client"),
                _ => {}
            }
        }
        assert!(harness.stored_events(&id).iter().any(|event| matches!(event,
            SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended {
                outcome: TurnOutcome::Cancelled, ..
            }))
        )));
        client.handle.request(CloseSessionRequest::new(id.clone())).await.unwrap();
        assert_stopped_turn(&harness, &id);
        assert!(client.handle.prompt(PromptRequest::new(id, vec!["closed".into()])).await.is_err());
        server.shutdown().await;
        harness.shutdown().await;
    })).await;
}

#[tokio::test]
async fn failed_handshake_and_graceful_close_release_admission() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let server = harness.serve_websocket().await;
            let mut detached = server.subscribe();
            let seen = *detached.borrow();
            let mut invalid = TcpStream::connect(server.address).await.unwrap();
            invalid.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            let mut response = Vec::new();
            invalid.read_to_end(&mut response).await.unwrap();
            detached.wait_for(|g| *g > seen).await.unwrap();

            let seen = *detached.borrow();
            let (mut socket, _) = connect_async(server.url()).await.unwrap();
            socket.send(Message::Close(None)).await.unwrap();
            assert!(matches!(socket.next().await, Some(Ok(Message::Close(_)))));
            detached.wait_for(|g| *g > seen).await.unwrap();

            let (socket, _) = connect_async(server.url()).await.unwrap();
            let client = connect_acp_client(WebSocketTransport::new(socket), initialize_request()).await.unwrap();
            assert_eq!(client.initialize_response.info.name, "Aether");
            server.shutdown().await;
            harness.shutdown().await;
        }))
        .await;
}

#[tokio::test]
async fn shutdown_stops_paused_runtime_and_pending_handshake() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, _release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut socket = TcpStream::connect(server.address).await.unwrap();
            assert_occupied(&server).await;
            assert_no_ended_turn(&harness, &id);
            let address = server.address;
            server.shutdown().await;
            assert_no_ended_turn(&harness, &id);
            harness.shutdown().await;
            assert_stopped_turn(&harness, &id);
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
            assert!(TcpStream::connect(address).await.is_err());
        }))
        .await;
}

#[tokio::test]
async fn dropping_running_server_closes_listener_and_active_connection() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, _release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut client = connect(&server, &id).await;
            expect_running(&mut client).await;
            let address = server.address;
            let mut detached = server.subscribe();
            let seen = *detached.borrow();

            server.abort().await;
            detached.wait_for(|g| *g > seen).await.unwrap();
            expect_closed(&mut client).await;
            assert!(TcpStream::connect(address).await.is_err());
            assert_no_ended_turn(&harness, &id);
            harness.reconnect().await;
            harness
                .client_cx
                .send_request(ResumeSessionRequest::new(id.clone(), AbsolutePath::new("/tmp")))
                .block_task()
                .await
                .unwrap();

            harness.shutdown().await;
            assert_stopped_turn(&harness, &id);
        }))
        .await;
}

#[tokio::test]
async fn dropping_running_server_aborts_pending_handshake() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let server = harness.serve_websocket().await;
            let mut socket = TcpStream::connect(server.address).await.unwrap();
            assert_occupied(&server).await;
            let address = server.address;

            server.abort().await;
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
            assert!(TcpStream::connect(address).await.is_err());
            harness.shutdown().await;
        }))
        .await;
}

#[tokio::test]
async fn networking_shutdown_detaches_without_stopping_the_host() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut first = connect(&server, &id).await;
            expect_running(&mut first).await;
            let address = server.address;

            server.shutdown().await;
            expect_closed(&mut first).await;
            assert!(TcpStream::connect(address).await.is_err());
            assert_no_ended_turn(&harness, &id);

            let server = harness.serve_websocket().await;
            let mut second = connect(&server, &id).await;
            expect_running(&mut second).await;
            release.notify_one();
            expect_completed(&mut second, &id, Some(StopReason::EndTurn)).await;
            assert_persisted_response(&harness, &id);
            server.shutdown().await;
            harness.shutdown().await;
            assert_eq!(harness.live_runtime_count(), 0);
        }))
        .await;
}

#[tokio::test]
async fn dropping_test_server_does_not_shut_down_the_host() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, _release) = start_paused_turn(&mut harness).await;
            let server = harness.serve_websocket().await;
            let mut client = connect(&server, &id).await;
            expect_running(&mut client).await;
            let address = server.address;
            drop(server);
            expect_closed(&mut client).await;
            assert!(TcpStream::connect(address).await.is_err());
            assert_no_ended_turn(&harness, &id);
            harness.shutdown().await;
            assert_stopped_turn(&harness, &id);
        }))
        .await;
}

#[tokio::test]
async fn networking_shutdown_cancels_pending_session_startup_and_allows_reconnect() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let mut harness = AcpTestHarness::start().await;
            let mut startup = harness.pause_next_runtime();
            let server = harness.serve_websocket().await;
            let (socket, _) = connect_async(server.url()).await.unwrap();
            let client = connect_acp_client(WebSocketTransport::new(socket), initialize_request()).await.unwrap();
            let pending = tokio::task::spawn_local(async move {
                client.handle.new_session(NewSessionRequest::new(AbsolutePath::new("/tmp"))).await
            });
            startup.wait_until_started().await;
            server.shutdown().await;
            assert!(pending.await.unwrap().is_err());
            assert_eq!(harness.live_runtime_count(), 0);

            let server = harness.serve_websocket().await;
            let (socket, _) = connect_async(server.url()).await.unwrap();
            let client = connect_acp_client(WebSocketTransport::new(socket), initialize_request()).await.unwrap();
            client.handle.new_session(NewSessionRequest::new(AbsolutePath::new("/tmp"))).await.unwrap();
            assert_eq!(harness.live_runtime_count(), 1);
            server.shutdown().await;
            harness.shutdown().await;
            assert_eq!(harness.live_runtime_count(), 0);
        }))
        .await;
}

async fn assert_occupied(server: &AcpWebSocketTestServer) {
    let error = connect_async(server.url()).await.unwrap_err();
    let Error::Http(response) = error else { panic!("expected an HTTP conflict, got {error}") };
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.body().as_deref(), Some(b"client already attached".as_slice()));
}

async fn connect(server: &AcpWebSocketTestServer, id: &SessionId) -> AcpClient {
    let mut request = format!("{}/acp?source=test", server.url()).into_client_request().unwrap();
    request.headers_mut().insert("x-test-gateway", "opaque-value".parse().unwrap());
    let (socket, _) = connect_async(request).await.unwrap();
    let client = connect_acp_client(WebSocketTransport::new(socket), initialize_request()).await.unwrap();
    client
        .handle
        .resume_session_with_replay(ResumeSessionRequest::new(id.clone(), AbsolutePath::new("/tmp")))
        .await
        .unwrap();
    client
}

async fn expect_running(client: &mut AcpClient) {
    loop {
        match client.event_rx.recv().await.expect("connection remains open") {
            AcpEvent::SessionUpdate(update) => match update.update {
                SessionUpdate::StateUpdate(StateUpdate::Running(_)) => return,
                SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => panic!("turn was interrupted"),
                _ => {}
            },
            AcpEvent::ConnectionClosed => panic!("connection closed before running"),
            _ => {}
        }
    }
}

async fn expect_completed(client: &mut AcpClient, id: &SessionId, stop_reason: Option<StopReason>) {
    let mut completed = false;
    loop {
        match client.event_rx.recv().await.expect("connection remains open") {
            AcpEvent::SessionUpdate(update) => {
                assert_eq!(&update.session_id, id);
                match update.update {
                    SessionUpdate::AgentMessage(message) => {
                        completed |= message.content.value().is_some_and(|content| {
                            content.iter().any(|block| {
                            matches!(block, ContentBlock::Text(text) if text.text == "before gate after gate")
                        })
                        });
                    }
                    SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) => {
                        assert!(completed, "completed message must precede idle");
                        assert_eq!(idle.stop_reason, stop_reason);
                        return;
                    }
                    _ => {}
                }
            }
            AcpEvent::ConnectionClosed => panic!("connection closed before completion"),
            _ => {}
        }
    }
}

async fn expect_closed(client: &mut AcpClient) {
    while let Some(event) = client.event_rx.recv().await {
        if matches!(event, AcpEvent::ConnectionClosed) {
            return;
        }
    }
    panic!("expected connection closed event");
}

fn assert_persisted_response(harness: &AcpTestHarness, id: &SessionId) {
    let events = harness.stored_events(id);
    assert_eq!(events.iter().filter(|event| matches!(event, SessionEvent::User(UserEvent::Message { .. }))).count(), 1);
    let texts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. })) => {
                Some(chunk.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["before gate after gate"]);
}
