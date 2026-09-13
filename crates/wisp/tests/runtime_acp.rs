#![cfg(feature = "testing")]

use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::testing::duplex_pair;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AgentCapabilities, AuthMethodId, CancelSessionNotification, ContentBlock, Implementation, InitializeRequest,
    InitializeResponse, LoginAuthRequest, LoginAuthResponse, NewSessionRequest, NewSessionResponse, PromptCapabilities,
    PromptImageCapabilities, PromptRequest, PromptResponse, ReplayFrom, ResumeSessionRequest, ResumeSessionResponse,
    SessionCapabilities, SessionConfigId, SessionConfigOption, SessionConfigOptionValue, SessionConfigSelectOption,
    SessionId, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, TextContent,
};
use agent_client_protocol::{Agent, Responder};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::task::{LocalSet, spawn_local};
use wisp::command::{AgentCommand, Command, CommandResult, FailedCommand};
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
                let (session, mut peer) = connect(capabilities.clone()).await;
                let initialize = peer.initialize.recv().await.unwrap();
                assert_eq!(initialize.protocol_version, ProtocolVersion::V2);
                assert_eq!(initialize.info.name, "wisp");
                let elicitation = initialize.capabilities.elicitation.unwrap();
                assert!(elicitation.form.is_some());
                assert!(elicitation.url.is_some());
                assert_eq!(session.agent_name, "Fake agent");
                assert_eq!(session.session_id, SessionId::new("new"));
                assert_eq!(session.config_options[0].config_id, SessionConfigId::new("model"));
                assert_eq!(session.prompt_capabilities.image.is_some(), capabilities.is_some());
                session.client_handle.disconnect().await;
            }
        })
        .await;
}

#[tokio::test]
async fn runtime_login_and_config_requests_use_v2_payloads() {
    LocalSet::new().run_until(async {
        let (session, mut peer) = connect(None).await;
        let mut dispatcher = CommandDispatcher::new(session.client_handle.clone());
        dispatcher.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AuthenticationCompleted { method_id }) if method_id == "provider"));
        assert_eq!(peer.login.recv().await.unwrap().method_id, AuthMethodId::new("provider"));

        for (value, expected) in [
            (SessionConfigOptionValue::from("fast"), serde_json::json!({"type": "id", "value": "fast"})),
            (SessionConfigOptionValue::from(true), serde_json::json!({"type": "boolean", "value": true})),
        ] {
            dispatcher.dispatch(Command::Agent(AgentCommand::SetConfigOption {
                session_id: session.session_id.clone(), config_id: "setting".into(), value,
            }));
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::ConfigOptionsUpdated(_))));
            let request = peer.config.recv().await.unwrap();
            let wire = serde_json::to_value(request).unwrap();
            assert_eq!(wire["configId"], "setting");
            assert_eq!(wire["type"], expected["type"]);
            assert_eq!(wire["value"], expected["value"]);
        }
        session.client_handle.disconnect().await;
    }).await;
}

#[tokio::test]
async fn runtime_replays_sequentially_and_rejects_restore_during_a_turn() {
    LocalSet::new().run_until(async {
        let (mut session, mut peer) = connect(None).await;
        let mut dispatcher = CommandDispatcher::new(session.client_handle.clone());
        for id in ["first", "second"] {
            dispatcher.dispatch(Command::Agent(AgentCommand::ResumeSession { session_id: id.into(), cwd: PathBuf::from("/workspace") }));
            let (request, responder) = peer.resume.recv().await.unwrap();
            assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
            assert!(matches!(session.client_handle.resume_session(ResumeSessionRequest::new("other", "/workspace")).await, Err(AcpClientError::Busy)));
            assert!(matches!(session.client_handle.prompt(PromptRequest::new("other", vec![])).await, Err(AcpClientError::Busy)));
            responder.respond(ResumeSessionResponse::new(vec![])).unwrap();
            assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AgentCommandAccepted)));
            assert!(matches!(session.event_rx.recv().await, Some(AcpEvent::SessionResumed(resumed)) if resumed.session_id == SessionId::new(id)));
        }
        let handle = session.client_handle.clone();
        let plain_resume = spawn_local(async move {
            handle.resume_session(ResumeSessionRequest::new("second", "/workspace")).await
        });
        let (request, responder) = peer.resume.recv().await.unwrap();
        assert!(request.replay_from.is_none());
        assert!(matches!(session.client_handle.resume_session_with_replay(ResumeSessionRequest::new("other", "/workspace")).await, Err(AcpClientError::Busy)));
        responder.respond(ResumeSessionResponse::new(vec![])).unwrap();
        plain_resume.await.unwrap().unwrap();
        assert!(session.event_rx.try_recv().is_err());

        dispatcher.dispatch(Command::Agent(AgentCommand::Prompt {
            session_id: "second".into(), text: "hello".into(), content: None,
        }));
        let (request, responder) = peer.prompt.recv().await.unwrap();
        assert_eq!(request.prompt, vec![ContentBlock::Text(TextContent::new("hello"))]);
        responder.respond(PromptResponse::new()).unwrap();
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AgentCommandAccepted)));
        dispatcher.dispatch(Command::Agent(AgentCommand::ResumeSession { session_id: "other".into(), cwd: PathBuf::from("/workspace") }));
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::Failed { command: FailedCommand::ResumeSession, .. })));
        dispatcher.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AuthenticationCompleted { .. })));
        dispatcher.dispatch(Command::Agent(AgentCommand::Cancel { session_id: "second".into() }));
        assert!(matches!(dispatcher.next_result().await, Some(CommandResult::AgentCommandAccepted)));
        assert_eq!(peer.cancel.recv().await.unwrap().session_id, SessionId::new("second"));
        session.client_handle.disconnect().await;
    }).await;
}

struct Peer {
    initialize: mpsc::UnboundedReceiver<InitializeRequest>,
    login: mpsc::UnboundedReceiver<LoginAuthRequest>,
    config: mpsc::UnboundedReceiver<SetSessionConfigOptionRequest>,
    resume: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    prompt: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    cancel: mpsc::UnboundedReceiver<CancelSessionNotification>,
}

async fn connect(capabilities: Option<SessionCapabilities>) -> (Session, Peer) {
    let (initialize_tx, initialize) = mpsc::unbounded_channel();
    let (login_tx, login) = mpsc::unbounded_channel();
    let (config_tx, config) = mpsc::unbounded_channel();
    let (resume_tx, resume) = mpsc::unbounded_channel();
    let (prompt_tx, prompt) = mpsc::unbounded_channel();
    let (cancel_tx, cancel) = mpsc::unbounded_channel();
    let agent = Agent
        .v2()
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                initialize_tx.send(request).unwrap();
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V2, Implementation::new("Fake agent", "1"))
                        .capabilities(AgentCapabilities::new().session(capabilities.clone())),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: NewSessionRequest, responder, _cx| {
                assert_eq!(request.cwd.0, PathBuf::from("/workspace"));
                assert!(request.mcp_servers.is_empty());
                responder.respond(NewSessionResponse::new(
                    "new",
                    vec![SessionConfigOption::select(
                        "model",
                        "Model",
                        "fast",
                        vec![SessionConfigSelectOption::new("fast", "Fast")],
                    )],
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoginAuthRequest, responder, _cx| {
                login_tx.send(request).unwrap();
                responder.respond(LoginAuthResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _cx| {
                config_tx.send(request).unwrap();
                responder.respond(SetSessionConfigOptionResponse::new(vec![]))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, _cx| {
                resume_tx.send((request, responder)).unwrap();
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, _cx| {
                prompt_tx.send((request, responder)).unwrap();
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelSessionNotification, _cx| {
                cancel_tx.send(notification).unwrap();
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        );
    let (agent_transport, client_transport) = duplex_pair();
    spawn_local(agent.connect_to(agent_transport));
    let session = Session::connect_to(client_transport, PathBuf::from("/workspace")).await.unwrap();
    (session, Peer { initialize, login, config, resume, prompt, cancel })
}
