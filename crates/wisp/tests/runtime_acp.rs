#![cfg(feature = "testing")]

use acp_utils::testing::{duplex_pair, idle_notification, running_notification};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AgentCapabilities, AuthMethodId, CancelSessionNotification, ContentBlock, Implementation, InitializeRequest,
    LoginAuthRequest, NewSessionResponse, PromptCapabilities, PromptImageCapabilities, PromptRequest, PromptResponse,
    ReplayFrom, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionConfigId, SessionConfigOption,
    SessionConfigOptionValue, SessionConfigSelectOption, SessionId, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::schema::v2::{
    ContentChunk, SessionUpdate, StopReason, UpdateSessionNotification, UserMessage,
};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::task::{LocalSet, spawn_local};
use wisp::command::{AgentCommand, Command, CommandResult};
use wisp::runtime::CommandDispatcher;
use wisp::session::Session;

#[tokio::test]
async fn connection_negotiates_v2_and_projects_optional_capabilities() {
    LocalSet::new()
        .run_until(async {
            for capabilities in [
                None,
                Some(
                    SessionCapabilities::new().prompt(PromptCapabilities::new().image(PromptImageCapabilities::new())),
                ),
            ] {
                let (session, mut peer) = Box::pin(connect(capabilities.clone())).await;
                let initialize = peer.initialize.recv().await.unwrap();
                assert_eq!(initialize.protocol_version, ProtocolVersion::V2);
                assert_eq!(initialize.info.name, "wisp");
                let elicitation = initialize.capabilities.elicitation.unwrap();
                assert!(elicitation.form.is_some());
                assert!(elicitation.url.is_some());
                assert_eq!(session.client.agent_name(), "Fake agent");
                assert_eq!(session.response.session_id, SessionId::new("new"));
                assert_eq!(session.response.config_options[0].config_id, SessionConfigId::new("model"));
                assert_eq!(
                    session.client.prompt_capabilities().is_some_and(|prompt| prompt.image.is_some()),
                    capabilities.is_some()
                );
                session.client.handle.disconnect().await;
            }
        })
        .await;
}

#[tokio::test]
async fn runtime_login_and_config_requests_use_v2_payloads() {
    LocalSet::new().run_until(async {
        let (session, mut peer) = connect(None).await;
        let mut dispatcher = CommandDispatcher::new(session.client.handle.clone());
        dispatcher.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AuthenticationCompleted { method_id, result: Ok(_) }) if method_id == "provider"));
        assert_eq!(peer.login.recv().await.unwrap().method_id, AuthMethodId::new("provider"));

        let conversation_id = wisp::testing::TestUi::new().app().conversation_id();
        for (value, expected) in [
            (SessionConfigOptionValue::from("fast"), serde_json::json!({"type": "id", "value": "fast"})),
            (SessionConfigOptionValue::from(true), serde_json::json!({"type": "boolean", "value": true})),
        ] {
            dispatcher.dispatch(Command::Agent(AgentCommand::SetConfigOption {
                conversation_id, session_id: session.response.session_id.clone(), config_id: "setting".into(), value,
            }));
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::ConfigOptionsUpdated { .. })));
            let request = peer.config.recv().await.unwrap();
            let wire = serde_json::to_value(request).unwrap();
            assert_eq!(wire["configId"], "setting");
            assert_eq!(wire["type"], expected["type"]);
            assert_eq!(wire["value"], expected["value"]);
        }
        session.client.handle.disconnect().await;
    }).await;
}

#[tokio::test]
async fn accepted_turn_finishes_the_ui_only_after_matching_live_idle() {
    LocalSet::new()
        .run_until(async {
            for reason in [Some(StopReason::EndTurn), Some(StopReason::Cancelled), None] {
                let (mut session, mut peer) = connect(None).await;
                let mut ui = Box::new(wisp::testing::TestUi::new());
                ui.deliver_result(CommandResult::NewSession(Ok(NewSessionResponse::new("new"))));
                ui.submit("hello");
                let mut dispatcher = CommandDispatcher::new(session.client.handle.clone());
                for command in ui.executor_mut().take_commands() {
                    dispatcher.dispatch(command);
                }
                let (_, responder) = peer.prompt.recv().await.unwrap();
                responder.respond(PromptResponse::new()).unwrap();
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
async fn runtime_replays_sequentially_without_owning_turn_policy() {
    LocalSet::new()
        .run_until(async {
            let (mut session, mut peer) = connect(None).await;
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone());
            for id in ["first", "second"] {
                dispatcher.dispatch(Command::Agent(AgentCommand::ResumeSession {
                    session_id: id.into(),
                    cwd: PathBuf::from("/workspace"),
                }));
                let (request, responder) = peer.resume.recv().await.unwrap();
                assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
                responder.respond(ResumeSessionResponse::new()).unwrap();
                assert!(matches!(
                    dispatcher.next_result().await,
                    Some(CommandResult::ResumeSession { result: Ok(_), .. })
                ));
                assert!(session.client.event_rx.try_recv().is_err());
            }
            let handle = session.client.handle.clone();
            let plain_resume =
                spawn_local(
                    async move { handle.resume_session(ResumeSessionRequest::new("second", "/workspace")).await },
                );
            let (request, responder) = peer.resume.recv().await.unwrap();
            assert!(request.replay_from.is_none());
            responder.respond(ResumeSessionResponse::new()).unwrap();
            plain_resume.await.unwrap().unwrap();
            assert!(session.client.event_rx.try_recv().is_err());

            dispatcher.dispatch(Command::Agent(AgentCommand::Prompt {
                session_id: "second".into(),
                text: "hello".into(),
                content: None,
            }));
            let (request, responder) = peer.prompt.recv().await.unwrap();
            assert_eq!(request.prompt, vec![ContentBlock::Text(TextContent::new("hello"))]);
            responder.respond(PromptResponse::new()).unwrap();
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::Prompt(Ok(_)))));
            dispatcher.dispatch(Command::Agent(AgentCommand::ResumeSession {
                session_id: "other".into(),
                cwd: PathBuf::from("/workspace"),
            }));
            let (_, responder) = peer.resume.recv().await.unwrap();
            responder.respond(ResumeSessionResponse::new()).unwrap();
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::ResumeSession { result: Ok(_), .. })));
            assert!(session.client.event_rx.try_recv().is_err());
            dispatcher.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AuthenticationCompleted { .. })));
            assert!(matches!(
                dispatcher.dispatch(Command::Agent(AgentCommand::Cancel { session_id: "second".into() })),
                Some(CommandResult::Cancel(Ok(())))
            ));
            assert_eq!(peer.cancel.recv().await.unwrap().session_id, SessionId::new("second"));
            session.client.handle.disconnect().await;
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
            let (server, transport) = duplex_pair();
            spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(transport, None).await.unwrap();
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone());
            let (mut ui, mut events) = wisp::testing::TestUiBuilder::new().build_from_session(session);
            ui.submit("keep running");
            for command in ui.take_commands() {
                dispatcher.dispatch(command);
            }
            let (_, responder) = requests.prompt.recv().await.unwrap();
            responder.respond(PromptResponse::new()).unwrap();
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
            let (server, transport) = duplex_pair();
            let driver = spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(transport, None).await.unwrap();
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone());
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
            let (server, transport) = duplex_pair();
            spawn_local(agent.agent().connect_to(server));
            let session = Session::connect_remote_to(transport, None).await.unwrap();
            assert_eq!(requests.new_session.recv().await.unwrap().cwd.0, cwd);
            assert_eq!(session.working_dir, cwd);
            assert_eq!(session.response.session_id, SessionId::new("created"));
            assert!(requests.resume.try_recv().is_err());
            assert_eq!(session.workspace_access, wisp::session::WorkspaceAccess::Remote);
            let handle = session.client.handle.clone();
            let (mut ui, _) = wisp::testing::TestUiBuilder::new().dimensions(120, 24).build_from_session(session);
            assert!(ui.take_commands().is_empty(), "remote startup must not resolve the local workspace");
            ui.assert_viewport_contains("remote: /remote-only/workspace");
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
                let (server, transport) = duplex_pair();
                spawn_local(agent.agent().connect_to(server));
                let startup = spawn_local(Session::connect_remote_to(transport, requested.map(SessionId::new)));
                let (request, responder) = requests.resume.recv().await.unwrap();
                assert_eq!(request.session_id, SessionId::new(selected));
                assert_eq!(request.cwd.0, PathBuf::from(cwd));
                assert!(request.mcp_servers.is_empty());
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
                assert!(ui.take_commands().is_empty());
                let history = events.try_recv().expect("replay queued before response");
                let state = events.try_recv().expect("state queued before response");
                ui.acp_event(history);
                ui.acp_event(state);
                assert!(ui.conversation_text().contains("history"));
                assert_eq!(ui.app().waiting_for_response(), running);
                connection.send_notification(idle_notification(selected, Some(StopReason::EndTurn))).unwrap();
                ui.acp_event(events.recv().await.unwrap());
                assert!(!ui.app().waiting_for_response());
                handle.disconnect().await;
            }
        })
        .await;
}

#[tokio::test]
async fn remote_startup_rejects_missing_contract_without_creating_a_session() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                .new_session_response(NewSessionResponse::new("must-not-create"))
                .capture();
            let (server, transport) = duplex_pair();
            spawn_local(agent.agent().connect_to(server));
            assert!(matches!(
                Session::connect_remote_to(transport, None).await,
                Err(wisp::error::AppError::MissingRemoteContract)
            ));
            assert!(requests.new_session.try_recv().is_err());
            assert!(requests.resume.try_recv().is_err());
        })
        .await;
}

#[tokio::test]
async fn remote_startup_unknown_explicit_session_does_not_fall_back_to_live() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
                .remote_server(&acp_utils::notifications::RemoteServerInfo {
                    cwd: "/remote/live".into(),
                    session_id: Some("live".into()),
                })
                .capture();
            let (server, transport) = duplex_pair();
            spawn_local(agent.agent().connect_to(server));
            assert!(matches!(
                Session::connect_remote_to(transport, Some("missing".into())).await,
                Err(wisp::error::AppError::Acp(_))
            ));
            assert!(requests.new_session.try_recv().is_err());
            assert!(requests.resume.try_recv().is_err());
        })
        .await;
}

struct Peer {
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    initialize: mpsc::UnboundedReceiver<InitializeRequest>,
    login: mpsc::UnboundedReceiver<LoginAuthRequest>,
    config: mpsc::UnboundedReceiver<SetSessionConfigOptionRequest>,
    resume: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    prompt: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
}

async fn connect(capabilities: Option<SessionCapabilities>) -> (Session, Peer) {
    let (agent, mut requests) = acp_utils::testing::FakeAgent::default()
        .agent_info(Implementation::new("Fake agent", "1"))
        .capabilities(AgentCapabilities::new().session(capabilities))
        .new_session_response(NewSessionResponse::new("new").config_options(vec![SessionConfigOption::select(
            "model",
            "Model",
            "fast",
            vec![SessionConfigSelectOption::new("fast", "Fast")],
        )]))
        .login_method("provider")
        .capture();
    let (agent_transport, client_transport) = duplex_pair();
    spawn_local(agent.agent().connect_to(agent_transport));
    let session = Session::connect_to(client_transport, PathBuf::from("/workspace")).await.unwrap();
    let created = requests.new_session.recv().await.unwrap();
    assert_eq!(created.cwd.0, PathBuf::from("/workspace"));
    assert!(created.mcp_servers.is_empty());
    let connection = requests.connection.recv().await.unwrap();
    (
        session,
        Peer {
            connection,
            initialize: requests.initialize,
            login: requests.login,
            config: requests.config,
            resume: requests.resume,
            prompt: requests.prompt,
            cancel: requests.cancel,
        },
    )
}
