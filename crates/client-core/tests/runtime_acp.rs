use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::conversation::Conversation;
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::{
    FakeAgent, FakeAgentRequests, idle_notification, running_notification, select, session_preview, texts,
};
use agent_client_protocol::Error;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AgentCapabilities, AuthMethodId, ContentBlock, ContentChunk, Implementation, NewSessionResponse,
    PromptCapabilities, PromptImageCapabilities, PromptRequest, PromptResponse, ReplayFrom, ResumeSessionRequest,
    ResumeSessionResponse, SessionCapabilities, SessionConfigId, SessionConfigOption, SessionConfigOptionValue,
    SessionId, SessionUpdate, StopReason, TextContent, UpdateSessionNotification,
};
use client_core::command::RequestId;
use client_core::command::{AgentCommand, Command, CommandResult, PromptRejection, Reply};
use client_core::composer::submission::Submission;
use client_core::connection::Connection;
use client_core::connection::ConnectionError;
use client_core::runtime::{Input, SessionRuntime};
use client_core::session_core::SessionCoreConfig;
use client_core::session_model::WorkspaceAccess;
use client_core::testing::{LiveSession, client_info};
use std::path::{Path, PathBuf};
use tokio::task::LocalSet;

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
                let (config, mut runtime, mut peer) = Box::pin(connect(capabilities.clone())).await;
                let initialize = peer.initialize.recv().await.unwrap();
                assert_eq!(initialize.protocol_version, ProtocolVersion::V2);
                assert_eq!(initialize.info, client_info());
                let elicitation = initialize.capabilities.elicitation.unwrap();
                assert!(elicitation.form.is_some());
                assert!(elicitation.url.is_some());
                assert_eq!(config.initialize_response.info.name, "Fake agent");
                assert_eq!(config.session_response.session_id, SessionId::new("new"));
                assert_eq!(config.session_response.config_options[0].config_id, SessionConfigId::new("model"));
                assert_eq!(config.workspace_access, WorkspaceAccess::Local);
                let prompt = config.initialize_response.capabilities.session.and_then(|session| session.prompt);
                assert_eq!(prompt.is_some_and(|prompt| prompt.image.is_some()), capabilities.is_some());
                runtime.shutdown().await;
            }
        })
        .await;
}

#[tokio::test]
async fn connecting_without_a_new_session_lets_the_host_resume_a_saved_one() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = FakeAgent::new("deferred").hold::<ResumeSessionRequest>().capture();
            let client_transport = agent.serve();

            let connection = Connection::connect_to(client_info(), client_transport).await.unwrap();
            assert_eq!(requests.initialize.recv().await.unwrap().info, client_info());
            assert!(requests.new_session.try_recv().is_err(), "connecting alone must not create a session");

            let resume = tokio::task::spawn_local(connection.open(Some("saved".into()), PathBuf::from("/workspace")));
            let (request, responder) = requests.held::<ResumeSessionRequest>().recv().await.unwrap();
            assert_eq!(request.session_id, SessionId::new("saved"));
            assert_eq!(request.cwd.0, PathBuf::from("/workspace"));
            assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
            responder.respond(ResumeSessionResponse::new()).unwrap();
            let (config, mut runtime) = resume.await.unwrap().unwrap().into_core();
            assert!(requests.new_session.try_recv().is_err(), "resuming must not create a session");
            assert_eq!(config.session_response.session_id, SessionId::new("saved"));
            assert_eq!(config.working_dir, PathBuf::from("/workspace"));
            assert_eq!(config.workspace_access, WorkspaceAccess::Local);
            runtime.shutdown().await;
        })
        .await;
}

#[tokio::test]
async fn runtime_login_and_config_requests_use_v2_payloads() {
    Box::pin(LocalSet::new().run_until(async {
        let (config, mut runtime, mut peer) = connect(None).await;
        runtime.dispatch(Command::Agent(AgentCommand::Authenticate { method_id: "provider".into() }));
        assert!(matches!(
            next_result(&mut runtime).await,
            CommandResult::Reply(Reply::AuthenticationCompleted { method_id, result: Ok(_) }) if method_id == "provider"
        ));
        assert_eq!(peer.login.recv().await.unwrap().method_id, AuthMethodId::new("provider"));

        let session_id = config.session_response.session_id;
        let conversation_id = Conversation::new(session_id.clone()).id();
        for (value, expected) in [
            (SessionConfigOptionValue::from("fast"), serde_json::json!({"type": "id", "value": "fast"})),
            (SessionConfigOptionValue::from(true), serde_json::json!({"type": "boolean", "value": true})),
        ] {
            runtime.dispatch(Command::Agent(AgentCommand::SetConfigOption {
                conversation_id,
                session_id: session_id.clone(),
                config_id: "setting".into(),
                value,
            }));
            assert!(matches!(next_result(&mut runtime).await, CommandResult::ConfigOptionsUpdated { .. }));
            let request = peer.config.recv().await.unwrap();
            let wire = serde_json::to_value(request).unwrap();
            assert_eq!(wire["configId"], "setting");
            assert_eq!(wire["type"], expected["type"]);
            assert_eq!(wire["value"], expected["value"]);
        }
        runtime.shutdown().await;
    }))
    .await;
}

#[tokio::test]
async fn runtime_replays_sequentially_without_owning_turn_policy() {
    Box::pin(LocalSet::new().run_until(async {
        let (_, mut runtime, mut peer) = connect(None).await;
        for id in ["first", "second"] {
            runtime.dispatch(Command::Agent(AgentCommand::ResumeSession {
                session_id: id.into(),
                cwd: PathBuf::from("/workspace"),
            }));
            let (request, responder) = peer.held::<ResumeSessionRequest>().recv().await.unwrap();
            assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
            responder.respond(ResumeSessionResponse::new()).unwrap();
            assert!(matches!(next_result(&mut runtime).await, CommandResult::ResumeSession { result: Ok(_), .. }));
            assert!(runtime.try_next_event().is_none());
        }

        let request_id = RequestId::next();
        runtime.dispatch(Command::Agent(AgentCommand::Prompt {
            request_id,
            session_id: "second".into(),
            content: vec![ContentBlock::from("hello")],
        }));
        let (request, responder) = peer.held::<PromptRequest>().recv().await.unwrap();
        assert_eq!(request.prompt, vec![ContentBlock::Text(TextContent::new("hello"))]);
        responder.respond(PromptResponse::new("user-message")).unwrap();
        assert!(matches!(
            next_result(&mut runtime).await,
            CommandResult::Prompt { request_id: id, result: Ok(()) } if id == request_id
        ));
        for (error, rejection) in [
            (Error::request_cancelled(), PromptRejection::Cancelled),
            (
                Error::internal_error(),
                PromptRejection::Failed(AcpClientError::Protocol(Error::internal_error()).to_string()),
            ),
        ] {
            let request_id = RequestId::next();
            runtime.dispatch(Command::Agent(AgentCommand::Prompt {
                request_id,
                session_id: "second".into(),
                content: vec![ContentBlock::from("queued")],
            }));
            let (_, responder) = peer.held::<PromptRequest>().recv().await.unwrap();
            responder.respond_with_error(error).unwrap();
            assert!(matches!(
                next_result(&mut runtime).await,
                CommandResult::Prompt { request_id: id, result: Err(actual) } if id == request_id && actual == rejection
            ));
        }
        runtime.shutdown().await;
    }))
    .await;
}

#[tokio::test]
async fn remote_startup_rejects_missing_contract_without_creating_a_session() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) =
                FakeAgent::default().new_session_response(NewSessionResponse::new("must-not-create")).capture();
            let connection = Connection::connect_to(client_info(), agent.serve()).await.unwrap();
            assert!(matches!(connection.open_remote(None).await, Err(ConnectionError::MissingRemoteContract)));
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
                .remote_server(&RemoteServerInfo { cwd: "/remote/live".into(), session_id: Some("live".into()) })
                .capture();
            let connection = Connection::connect_to(client_info(), agent.serve()).await.unwrap();
            assert!(matches!(connection.open_remote(Some("missing".into())).await, Err(ConnectionError::Acp(_))));
            assert!(requests.new_session.try_recv().is_err());
            assert!(requests.resume.try_recv().is_err());
        })
        .await;
}

async fn next_result(runtime: &mut SessionRuntime) -> CommandResult {
    loop {
        if let Input::Result(result) = runtime.next().await {
            return result;
        }
    }
}

async fn connect(capabilities: Option<SessionCapabilities>) -> (SessionCoreConfig, SessionRuntime, FakeAgentRequests) {
    let (agent, mut requests) = FakeAgent::default()
        .agent_info(Implementation::new("Fake agent", "1"))
        .capabilities(AgentCapabilities::new().session(capabilities))
        .new_session_response(NewSessionResponse::new("new").config_options(model_options()))
        .login_method("provider")
        .hold::<PromptRequest>()
        .hold::<ResumeSessionRequest>()
        .capture();
    let connection = Connection::connect_to(client_info(), agent.serve()).await.unwrap();
    let (config, runtime) = connection.open(None, PathBuf::from("/workspace")).await.unwrap().into_core();
    let created = requests.new_session.recv().await.unwrap();
    assert_eq!(created.cwd.0, PathBuf::from("/workspace"));
    assert_eq!(created.mcp_servers.len(), 0);
    (config, runtime, requests)
}

#[tokio::test]
async fn a_remote_server_without_a_live_session_creates_one_in_its_workspace() {
    LocalSet::new()
        .run_until(async {
            let cwd = PathBuf::from("/remote-only/workspace");
            let (agent, mut requests) = FakeAgent::default()
                .remote_server(&RemoteServerInfo { cwd: cwd.clone(), session_id: None })
                .new_session_response(NewSessionResponse::new("created"))
                .capture();
            let connection = Connection::connect_to(client_info(), agent.serve()).await.unwrap();

            let (config, mut runtime) = connection.open_remote(None).await.unwrap().into_core();

            assert_eq!(requests.new_session.recv().await.unwrap().cwd.0, cwd);
            assert!(requests.resume.try_recv().is_err());
            assert_eq!(config.working_dir, cwd);
            assert_eq!(config.session_response.session_id, SessionId::new("created"));
            assert_eq!(config.workspace_access, WorkspaceAccess::Remote);
            runtime.shutdown().await;
        })
        .await;
}

#[tokio::test]
async fn a_remote_server_resumes_the_selected_session_with_its_replay_queued_for_the_core() {
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
                let (agent, mut requests) = FakeAgent::default()
                    .remote_server(&RemoteServerInfo {
                        cwd: "/remote/live".into(),
                        session_id: advertised.map(SessionId::new),
                    })
                    .session_preview(session_preview("saved", "/remote/saved"))
                    .hold::<ResumeSessionRequest>()
                    .capture();
                let connection = Connection::connect_to(client_info(), agent.serve()).await.unwrap();
                let startup = tokio::task::spawn_local(connection.open_remote(requested.map(SessionId::new)));

                let (request, responder) = requests.held::<ResumeSessionRequest>().recv().await.unwrap();
                assert_eq!(request.session_id, SessionId::new(selected));
                assert_eq!(request.cwd.0, PathBuf::from(cwd));
                assert!(matches!(request.replay_from, Some(ReplayFrom::Start(_))));
                let agent = requests.connection.recv().await.unwrap();
                let history = SessionUpdate::AgentMessageChunk(ContentChunk::new("history".into(), "reply"));
                agent.send_notification(UpdateSessionNotification::new(selected, history)).unwrap();
                let state = if running { running_notification(selected) } else { idle_notification(selected, None) };
                agent.send_notification(state).unwrap();
                responder.respond(ResumeSessionResponse::new().config_options(model_options())).unwrap();

                let mut live = LiveSession::start(startup.await.unwrap().unwrap());
                assert!(requests.new_session.try_recv().is_err());
                assert_eq!(live.core.session().session_id(), &SessionId::new(selected));
                assert_eq!(live.core.session().working_dir(), Path::new(cwd));
                live.run_until(|core| !core.conversation().items().is_empty()).await;
                live.run_until(|core| core.waiting_for_response() == running).await;
                assert_eq!(texts(live.core.conversation()), ["history"]);

                agent.send_notification(idle_notification(selected, Some(StopReason::EndTurn))).unwrap();
                live.run_until(|core| !core.waiting_for_response()).await;
                live.shutdown().await;
            }
        })
        .await;
}

#[tokio::test]
async fn a_prompt_the_agent_accepted_runs_until_its_turn_goes_idle() {
    Box::pin(LocalSet::new().run_until(async {
        for (stop_reason, cancel) in [(StopReason::EndTurn, false), (StopReason::Cancelled, true)] {
            let (agent, mut requests) = FakeAgent::new("new").capture();
            let mut live = LiveSession::connect(agent, "/workspace", None).await.unwrap();

            live.core.submit(Submission { text: "hello".into(), ..Submission::default() });
            live.run_until(|core| core.queued_prompts().is_empty() && !core.conversation().turn().is_idle()).await;
            assert!(live.core.waiting_for_response(), "acceptance is not completion");

            if cancel {
                live.core.cancel();
                live.run_until(|_| true).await;
                assert_eq!(requests.cancel.recv().await.unwrap().session_id, SessionId::new("new"));
                assert!(live.core.waiting_for_response(), "cancel is not completion");
            }
            requests.turns.reply("response");
            live.run_until(|core| texts(core.conversation()).len() == 2).await;
            assert!(live.core.waiting_for_response());

            requests.turns.end(stop_reason);
            live.run_until(|core| !core.waiting_for_response()).await;
            assert_eq!(texts(live.core.conversation()), ["hello", "response"]);
            live.shutdown().await;
        }
    }))
    .await;
}

#[tokio::test]
async fn shutting_down_ends_the_connection_without_cancelling_or_closing_the_session() {
    Box::pin(LocalSet::new().run_until(async {
        let (agent, mut requests) = FakeAgent::default()
            .remote_server(&RemoteServerInfo { cwd: "/remote/workspace".into(), session_id: None })
            .new_session_response(NewSessionResponse::new("new"))
            .capture();
        let mut live = LiveSession::connect(agent, "/", None).await.unwrap();
        live.core.submit(Submission { text: "keep running".into(), ..Submission::default() });
        live.run_until(|core| !core.queued_prompts().is_empty()).await;
        requests.prompt.recv().await.unwrap();

        live.runtime.shutdown().await;

        assert!(
            std::iter::from_fn(|| live.runtime.try_next_event())
                .any(|event| matches!(event, AcpEvent::ConnectionClosed)),
            "shutting down joins the agent connection"
        );
        assert!(requests.cancel.try_recv().is_err(), "shutting down is not cancelling");
        assert!(requests.close_session.try_recv().is_err(), "shutting down is not closing the session");
    }))
    .await;
}

fn model_options() -> Vec<SessionConfigOption> {
    vec![select("model", "Model", "fast", &[("fast", "Fast")])]
}
