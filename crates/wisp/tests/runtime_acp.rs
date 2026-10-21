#![cfg(feature = "testing")]

use acp_utils::testing::{idle_notification, running_notification};
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ContentChunk, Implementation, NewSessionResponse, PromptRequest, PromptResponse,
    ReplayFrom, ResumeSessionResponse, SessionConfigOption, SessionConfigSelectOption, SessionId, SessionUpdate,
    StopReason, UpdateSessionNotification, UserMessage,
};
use agent_client_protocol::{Channel, Responder};
use client_core::command::AgentCommand;
use client_core::runtime::CommandDispatcher;
use client_core::session::Session;
use client_core::workspace_status::WorkspaceAccess;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::task::{LocalSet, spawn_local};
use wisp::command::{Command, CommandResult};
use wisp::runtime::TuiHost;

#[tokio::test]
async fn accepted_turn_finishes_the_ui_only_after_matching_live_idle() {
    LocalSet::new()
        .run_until(async {
            for reason in [Some(StopReason::EndTurn), Some(StopReason::Cancelled), None] {
                let (mut session, mut peer) = connect().await;
                let mut ui = Box::new(wisp::testing::TestUi::new());
                ui.deliver_result(CommandResult::NewSession(Ok(NewSessionResponse::new("new"))));
                ui.submit("hello");
                let mut dispatcher = CommandDispatcher::new(session.client.handle.clone(), TuiHost);
                for command in ui.executor_mut().take_commands() {
                    dispatcher.dispatch(command);
                }
                let (_, responder) = peer.prompt.recv().await.unwrap();
                responder.respond(PromptResponse::new("user-message")).unwrap();
                ui.deliver_result(dispatcher.next_result().await.unwrap());
                assert!(ui.app().waiting_for_response(), "acceptance is not completion");
                if reason == Some(StopReason::Cancelled) {
                    let cancelled = dispatcher
                        .dispatch(Command::Agent(AgentCommand::Cancel { session_id: "new".into() }))
                        .expect("cancel is accepted immediately");
                    assert_eq!(peer.cancel.recv().await.unwrap().session_id, SessionId::new("new"));
                    ui.deliver_result(cancelled);
                    assert!(ui.app().waiting_for_response(), "cancel is not completion");
                }
                for notification in [
                    UpdateSessionNotification::new(
                        "new",
                        SessionUpdate::UserMessage(UserMessage::new("user").content(vec!["hello".into()])),
                    ),
                    running_notification("new"),
                    UpdateSessionNotification::new(
                        "new",
                        SessionUpdate::AgentMessageChunk(ContentChunk::new("response".into(), "reply")),
                    ),
                ] {
                    peer.connection.send_notification(notification).unwrap();
                    ui.acp_event(session.client.event_rx.recv().await.unwrap());
                    assert!(ui.app().waiting_for_response());
                }
                peer.connection.send_notification(idle_notification("new", reason)).unwrap();
                ui.acp_event(session.client.event_rx.recv().await.unwrap());
                assert!(!ui.app().waiting_for_response());
                let text = ui.conversation_text();
                assert_eq!(text.matches("hello").count(), 1, "{text}");
                assert!(text.contains("response"));
                session.client.handle.disconnect().await;
            }
        })
        .await;
}

#[tokio::test]
async fn remote_exit_joins_connection_without_cancel_or_close() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                .remote_server(&acp_utils::notifications::RemoteServerInfo {
                    cwd: "/remote/workspace".into(),
                    session_id: None,
                })
                .new_session_response(NewSessionResponse::new("new"))
                .capture();
            let (server, transport) = Channel::duplex();
            spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(wisp::client_info(), transport, None).await.unwrap();
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone(), TuiHost);
            let (mut ui, mut events) = wisp::testing::TestUiBuilder::new().build_from_session(session);
            ui.submit("keep running");
            for command in ui.take_commands() {
                dispatcher.dispatch(command);
            }
            let (_, responder) = requests.prompt.recv().await.unwrap();
            responder.respond(PromptResponse::new("user-message")).unwrap();
            ui.deliver_result(dispatcher.next_result().await.unwrap());
            ui.acp_event(running_notification("new").into());
            assert!(ui.app().waiting_for_response());
            let exit = crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('c'),
                crossterm::event::KeyModifiers::CONTROL,
            );
            ui.key(exit);
            assert!(!ui.app().exit_requested());
            ui.key(exit);
            assert!(matches!(ui.app().exit_result(), Some(Ok(()))));
            for command in ui.take_commands() {
                dispatcher.dispatch(command);
            }
            dispatcher.shutdown().await;
            assert!(
                matches!(events.try_recv(), Ok(acp_utils::client::AcpEvent::ConnectionClosed)),
                "runtime shutdown must join the ACP driver even when other handles remain"
            );
            assert!(requests.cancel.try_recv().is_err(), "exit is not cancellation");
            assert!(requests.close_session.try_recv().is_err(), "exit is not session close");
        })
        .await;
}

#[tokio::test]
async fn remote_connection_loss_exits_with_error_without_resending() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                .remote_server(&acp_utils::notifications::RemoteServerInfo {
                    cwd: "/remote/workspace".into(),
                    session_id: None,
                })
                .new_session_response(NewSessionResponse::new("new"))
                .capture();
            let (server, transport) = Channel::duplex();
            let driver = spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(wisp::client_info(), transport, None).await.unwrap();
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone(), TuiHost);
            let (mut ui, mut events) = wisp::testing::TestUiBuilder::new().build_from_session(session);
            ui.submit("acceptance is uncertain");
            for command in ui.take_commands() {
                dispatcher.dispatch(command);
            }
            let (_, _pending) = requests.prompt.recv().await.unwrap();
            driver.abort();
            let _ = driver.await;
            loop {
                let event = events.recv().await.expect("connection loss is reported");
                let closed = matches!(event, acp_utils::client::AcpEvent::ConnectionClosed);
                ui.acp_event(event);
                if closed {
                    break;
                }
            }
            let error = ui.app().exit_result().unwrap().unwrap_err();
            assert!(matches!(error, wisp::error::AppError::ConnectionLost));
            assert!(error.to_string().contains("agent may still be running"));
            assert!(error.to_string().contains("before resubmitting"));
            assert!(!ui.app().has_modal());
            assert!(!ui.app().needs_mouse_capture());
            assert!(ui.take_commands().is_empty(), "connection loss must not retry the prompt");
            dispatcher.shutdown().await;
            assert!(requests.prompt.try_recv().is_err());
            assert!(requests.cancel.try_recv().is_err());
            assert!(requests.close_session.try_recv().is_err());
        })
        .await;
}

#[tokio::test]
async fn remote_startup_creates_in_the_server_workspace() {
    LocalSet::new()
        .run_until(async {
            let cwd = PathBuf::from("/remote-only/workspace");
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                .remote_server(&acp_utils::notifications::RemoteServerInfo { cwd: cwd.clone(), session_id: None })
                .new_session_response(NewSessionResponse::new("created"))
                .capture();
            let (server, transport) = Channel::duplex();
            spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(wisp::client_info(), transport, None).await.unwrap();
            assert_eq!(requests.new_session.recv().await.unwrap().cwd.0, cwd);
            assert_eq!(session.working_dir, cwd);
            assert_eq!(session.response.session_id, SessionId::new("created"));
            assert!(requests.resume.try_recv().is_err());
            assert_eq!(session.workspace_access, WorkspaceAccess::Remote);
            let handle = session.client.handle.clone();
            let (mut ui, _) = wisp::testing::TestUiBuilder::new().dimensions(120, 24).build_from_session(session);
            assert!(
                ui.take_commands()
                    .iter()
                    .any(|command| matches!(command, Command::Agent(AgentCommand::FetchWorkspaceStatus { .. }))),
                "remote startup fetches agent-side workspace status"
            );
            ui.assert_viewport_contains("/remote-only/workspace");
            handle.disconnect().await;
        })
        .await;
}

#[tokio::test]
async fn remote_startup_resumes_selected_session_and_preserves_queued_replay() {
    LocalSet::new()
        .run_until(async {
            for (advertised, requested, running) in [
                (Some("live"), None, true),
                (Some("live"), Some("live"), false),
                (Some("live"), Some("saved"), true),
                (None, Some("saved"), false),
            ] {
                let selected = requested.unwrap_or("live");
                let cwd = if selected == "live" { "/remote/live" } else { "/remote/saved" };
                let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                    .remote_server(&acp_utils::notifications::RemoteServerInfo {
                        cwd: "/remote/live".into(),
                        session_id: advertised.map(SessionId::new),
                    })
                    .session_preview(acp_utils::notifications::SessionPreviewResponse {
                        session_id: "saved".into(),
                        cwd: "/remote/saved".into(),
                        created_at: String::new(),
                        model: "fast".into(),
                        selected_mode: None,
                        transcript: vec![],
                        tool_call_count: 0,
                        truncated: false,
                    })
                    .capture();
                let (server, transport) = Channel::duplex();
                spawn_local(agent.agent().connect_to(server));
                let startup = spawn_local(Session::connect_remote_to(
                    wisp::client_info(),
                    transport,
                    requested.map(SessionId::new),
                ));
                let (request, responder) = requests.resume.recv().await.unwrap();
                assert_eq!(request.session_id, SessionId::new(selected));
                assert_eq!(request.cwd.0, PathBuf::from(cwd));
                assert_eq!(request.mcp_servers.len(), 0);
                assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
                let connection = requests.connection.recv().await.unwrap();
                connection
                    .send_notification(UpdateSessionNotification::new(
                        selected,
                        SessionUpdate::AgentMessageChunk(ContentChunk::new("history".into(), "reply")),
                    ))
                    .unwrap();
                connection
                    .send_notification(if running {
                        running_notification(selected)
                    } else {
                        idle_notification(selected, None)
                    })
                    .unwrap();
                let options = vec![SessionConfigOption::select(
                    "model",
                    "Model",
                    "fast",
                    vec![SessionConfigSelectOption::new("fast", "Fast")],
                )];
                responder.respond(ResumeSessionResponse::new().config_options(options.clone())).unwrap();
                let session = startup.await.unwrap().unwrap();
                assert_eq!(session.response.session_id, SessionId::new(selected));
                assert_eq!(session.response.config_options, options);
                assert_eq!(session.working_dir, PathBuf::from(cwd));
                assert!(requests.new_session.try_recv().is_err());
                let handle = session.client.handle.clone();
                let (mut ui, mut events) =
                    wisp::testing::TestUiBuilder::new().dimensions(120, 24).build_from_session(session);
                assert_eq!(ui.app().session_id(), &SessionId::new(selected));
                assert!(
                    ui.take_commands()
                        .iter()
                        .any(|command| matches!(command, Command::Agent(AgentCommand::FetchWorkspaceStatus { .. }))),
                    "remote startup fetches agent-side workspace status"
                );
                let history = events.try_recv().expect("replay queued before response");
                let state = events.try_recv().expect("state queued before response");
                ui.acp_event(history);
                ui.acp_event(state);
                ui.assert_conversation_contains("history");
                assert_eq!(ui.app().waiting_for_response(), running);
                connection.send_notification(idle_notification(selected, Some(StopReason::EndTurn))).unwrap();
                ui.acp_event(events.recv().await.unwrap());
                assert!(!ui.app().waiting_for_response());
                handle.disconnect().await;
            }
        })
        .await;
}

struct Peer {
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    prompt: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
}

/// Connects a local session the way `wisp` does, asserting the identity Wisp
/// announces to the agent.
async fn connect() -> (Session, Peer) {
    let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
        .agent_info(Implementation::new("Fake agent", "1"))
        .new_session_response(NewSessionResponse::new("new"))
        .capture();
    let (agent_transport, client_transport) = Channel::duplex();
    spawn_local(agent.agent().connect_to(agent_transport));
    let session =
        Session::connect_to(wisp::client_info(), client_transport, PathBuf::from("/workspace")).await.unwrap();
    assert_eq!(requests.initialize.recv().await.unwrap().info.name, "wisp");
    assert_eq!(requests.new_session.recv().await.unwrap().cwd.0, PathBuf::from("/workspace"));
    let connection = requests.connection.recv().await.unwrap();
    (session, Peer { connection, prompt: requests.prompt, cancel: requests.cancel })
}
