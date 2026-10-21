use acp_utils::client::AcpClientError;
use acp_utils::conversation::Conversation;
use acp_utils::testing::FakeAgent;
use agent_client_protocol::Error;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AgentCapabilities, AuthMethodId, CancelSessionNotification, ContentBlock, Implementation, InitializeRequest,
    LoginAuthRequest, NewSessionResponse, PromptCapabilities, PromptImageCapabilities, PromptRequest, PromptResponse,
    ReplayFrom, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionConfigId, SessionConfigOption,
    SessionConfigOptionValue, SessionConfigSelectOption, SessionId, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{Channel, Responder};
use client_core::command::{AgentCommand, Command, CommandResult, PromptRejection};
use client_core::error::ClientError;
use client_core::request::RequestId;
use client_core::runtime::{CommandDispatcher, NoHost};
use client_core::session::{Connection, Session};
use client_core::workspace_status::WorkspaceAccess;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::task::{LocalSet, spawn_local};

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
                assert_eq!(initialize.info, client_info());
                let elicitation = initialize.capabilities.elicitation.unwrap();
                assert!(elicitation.form.is_some());
                assert!(elicitation.url.is_some());
                assert_eq!(session.client.agent_name(), "Fake agent");
                assert_eq!(session.response.session_id, SessionId::new("new"));
                assert_eq!(session.response.config_options[0].config_id, SessionConfigId::new("model"));
                assert_eq!(session.workspace_access, WorkspaceAccess::Local);
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
async fn connecting_without_a_new_session_defers_it_to_the_host() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) =
                FakeAgent::default().new_session_response(NewSessionResponse::new("deferred")).capture();
            let (agent_transport, client_transport) = Channel::duplex();
            spawn_local(agent.agent().connect_to(agent_transport));

            let connection =
                Connection::connect_to(client_info(), client_transport, PathBuf::from("/workspace")).await.unwrap();
            assert_eq!(requests.initialize.recv().await.unwrap().info, client_info());
            assert!(requests.new_session.try_recv().is_err(), "connecting alone must not create a session");

            let mut dispatcher = CommandDispatcher::new(connection.client.handle.clone(), NoHost);
            dispatcher.dispatch(Command::Agent(AgentCommand::ResumeSession {
                session_id: "saved".into(),
                cwd: connection.working_dir.clone(),
            }));
            let (request, responder) = requests.resume.recv().await.unwrap();
            assert_eq!(request.session_id, SessionId::new("saved"));
            assert_eq!(request.cwd.0, PathBuf::from("/workspace"));
            assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
            responder.respond(ResumeSessionResponse::new()).unwrap();
            assert!(matches!(
                dispatcher.next_result().await,
                Some(CommandResult::ResumeSession { session_id, result: Ok(_) }) if session_id == SessionId::new("saved")
            ));
            assert!(requests.new_session.try_recv().is_err(), "resuming must not create a session");

            let session = connection.new_session().await.unwrap();
            assert_eq!(requests.new_session.recv().await.unwrap().cwd.0, PathBuf::from("/workspace"));
            assert_eq!(session.response.session_id, SessionId::new("deferred"));
            assert_eq!(session.working_dir, PathBuf::from("/workspace"));
            assert_eq!(session.workspace_access, WorkspaceAccess::Local);
            dispatcher.shutdown().await;
        })
        .await;
}

#[tokio::test]
async fn runtime_login_and_config_requests_use_v2_payloads() {
    LocalSet::new()
        .run_until(async {
            let (session, mut peer) = connect(None).await;
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone(), NoHost);
            dispatcher.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
            assert!(matches!(
                dispatcher.next_result().await,
                Some(CommandResult::AuthenticationCompleted { method_id, result: Ok(_) }) if method_id == "provider"
            ));
            assert_eq!(peer.login.recv().await.unwrap().method_id, AuthMethodId::new("provider"));

            let conversation_id = Conversation::new().id();
            for (value, expected) in [
                (SessionConfigOptionValue::from("fast"), serde_json::json!({"type": "id", "value": "fast"})),
                (SessionConfigOptionValue::from(true), serde_json::json!({"type": "boolean", "value": true})),
            ] {
                dispatcher.dispatch(Command::Agent(AgentCommand::SetConfigOption {
                    conversation_id,
                    session_id: session.response.session_id.clone(),
                    config_id: "setting".into(),
                    value,
                }));
                assert!(matches!(dispatcher.next_result().await, Some(CommandResult::ConfigOptionsUpdated { .. })));
                let request = peer.config.recv().await.unwrap();
                let wire = serde_json::to_value(request).unwrap();
                assert_eq!(wire["configId"], "setting");
                assert_eq!(wire["type"], expected["type"]);
                assert_eq!(wire["value"], expected["value"]);
            }
            session.client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test]
async fn runtime_replays_sequentially_without_owning_turn_policy() {
    LocalSet::new()
        .run_until(async {
            let (mut session, mut peer) = connect(None).await;
            let mut dispatcher = CommandDispatcher::new(session.client.handle.clone(), NoHost);
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
                spawn_local(async move { handle.request(ResumeSessionRequest::new("second", "/workspace")).await });
            let (request, responder) = peer.resume.recv().await.unwrap();
            assert!(request.replay_from.is_none());
            responder.respond(ResumeSessionResponse::new()).unwrap();
            plain_resume.await.unwrap().unwrap();
            assert!(session.client.event_rx.try_recv().is_err());

            let request_id = RequestId::next();
            dispatcher.dispatch(Command::Agent(AgentCommand::Prompt {
                request_id,
                session_id: "second".into(),
                text: "hello".into(),
                content: None,
            }));
            let (request, responder) = peer.prompt.recv().await.unwrap();
            assert_eq!(request.prompt, vec![ContentBlock::Text(TextContent::new("hello"))]);
            responder.respond(PromptResponse::new("user-message")).unwrap();
            assert!(matches!(
                dispatcher.next_result().await,
                Some(CommandResult::Prompt { request_id: id, result: Ok(()) }) if id == request_id
            ));
            for (error, rejection) in [
                (Error::request_cancelled(), PromptRejection::Cancelled),
                (
                    Error::internal_error(),
                    PromptRejection::Failed(AcpClientError::Protocol(Error::internal_error()).to_string()),
                ),
            ] {
                let request_id = RequestId::next();
                dispatcher.dispatch(Command::Agent(AgentCommand::Prompt {
                    request_id,
                    session_id: "second".into(),
                    text: "queued".into(),
                    content: None,
                }));
                let (_, responder) = peer.prompt.recv().await.unwrap();
                responder.respond_with_error(error).unwrap();
                assert!(matches!(
                    dispatcher.next_result().await,
                    Some(CommandResult::Prompt { request_id: id, result: Err(actual) }) if id == request_id && actual == rejection
                ));
            }
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
async fn remote_startup_rejects_missing_contract_without_creating_a_session() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) =
                FakeAgent::default().new_session_response(NewSessionResponse::new("must-not-create")).capture();
            let (server, transport) = Channel::duplex();
            spawn_local(agent.agent().connect_to(server));
            assert!(matches!(
                Session::connect_remote_to(client_info(), transport, None).await,
                Err(ClientError::MissingRemoteContract)
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
            let (agent, mut requests) = FakeAgent::default()
                .remote_server(&acp_utils::notifications::RemoteServerInfo {
                    cwd: "/remote/live".into(),
                    session_id: Some("live".into()),
                })
                .capture();
            let (server, transport) = Channel::duplex();
            spawn_local(agent.agent().connect_to(server));
            assert!(matches!(
                Session::connect_remote_to(client_info(), transport, Some("missing".into())).await,
                Err(ClientError::Acp(_))
            ));
            assert!(requests.new_session.try_recv().is_err());
            assert!(requests.resume.try_recv().is_err());
        })
        .await;
}

struct Peer {
    initialize: mpsc::UnboundedReceiver<InitializeRequest>,
    login: mpsc::UnboundedReceiver<LoginAuthRequest>,
    config: mpsc::UnboundedReceiver<SetSessionConfigOptionRequest>,
    resume: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    prompt: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
}

fn client_info() -> Implementation {
    Implementation::new("client-core-test", "1")
}

async fn connect(capabilities: Option<SessionCapabilities>) -> (Session, Peer) {
    let (agent, mut requests) = FakeAgent::default()
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
    let (agent_transport, client_transport) = Channel::duplex();
    spawn_local(agent.agent().connect_to(agent_transport));
    let session = Session::connect_to(client_info(), client_transport, PathBuf::from("/workspace")).await.unwrap();
    let created = requests.new_session.recv().await.unwrap();
    assert_eq!(created.cwd.0, PathBuf::from("/workspace"));
    assert_eq!(created.mcp_servers.len(), 0);
    (
        session,
        Peer {
            initialize: requests.initialize,
            login: requests.login,
            config: requests.config,
            resume: requests.resume,
            prompt: requests.prompt,
            cancel: requests.cancel,
        },
    )
}
