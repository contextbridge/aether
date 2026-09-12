use acp::schema::v2::{AgentCapabilities, LoginAuthRequest, LoginAuthResponse};
use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, ReplayableEvent, ResumedSession, connect_acp_client};
use acp_utils::notifications::{
    ContextCompactionParams, PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse,
};
use acp_utils::testing::{duplex_pair, idle_notification, initialize_request, initialize_response};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ReplayFrom, ReplayFromStart, ResumeSessionRequest,
    ResumeSessionResponse, SessionId, SessionInfo, SessionUpdate, SetSessionConfigOptionRequest, StopReason,
    TextContent, UpdateSessionNotification,
};
use agent_client_protocol::{self as acp, Agent, Builder, Client, ConnectTo, HandleDispatchFrom, NullRun};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{
    Notify,
    mpsc::{error::TryRecvError, unbounded_channel},
};
use tokio::task::{LocalSet, spawn_local};

#[tokio::test(flavor = "current_thread")]
async fn cancel_reaches_the_agent_while_a_config_response_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let cancelled = Arc::new(Notify::new());
            let (prompt_tx, mut prompt_rx) = unbounded_channel();
            let (config_tx, mut config_rx) = unbounded_channel();

            let agent_builder = Agent
                .v2()
                .on_receive_request(
                    async |_req: InitializeRequest, responder, _cx| {
                        responder.respond(InitializeResponse::new(
                            ProtocolVersion::V2,
                            Implementation::new("Fake Agent", "0.0.0"),
                        ))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_req: NewSessionRequest, responder, _cx| {
                        responder.respond(NewSessionResponse::new(SessionId::new("sess-1")))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: PromptRequest, responder, _cx| {
                        prompt_tx.send(responder).unwrap();
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: SetSessionConfigOptionRequest, responder, _cx| {
                        config_tx.send(responder).unwrap();
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_notification(
                    {
                        let cancelled = Arc::clone(&cancelled);
                        async move |_n: CancelSessionNotification, _cx| {
                            cancelled.notify_one();
                            Ok(())
                        }
                    },
                    acp::on_receive_notification!(),
                );
            spawn_local(async move {
                let _ = agent_builder.connect_to(agent_transport).await;
            });

            let client = connect_acp_client(client_transport, acp_utils::testing::initialize_request())
                .await
                .expect("initialization succeeds");

            let created = client
                .handle
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .expect("session establishes");

            let session_id = created.session_id;
            let prompt_task_handle = client.handle.clone();
            let prompt_session_id = session_id.clone();
            spawn_local(async move {
                let _ = prompt_task_handle
                    .prompt(PromptRequest::new(prompt_session_id, vec![ContentBlock::Text(TextContent::new("hi"))]))
                    .await;
            });
            let prompt_responder = prompt_rx.recv().await.unwrap();
            let config_handle = client.handle.clone();
            let config_session_id = session_id.clone();
            spawn_local(async move {
                let _ = config_handle
                    .set_config_option(SetSessionConfigOptionRequest::new(config_session_id, "mode", "Plan"))
                    .await;
            });
            let config_responder = config_rx.recv().await.unwrap();
            client.handle.cancel(CancelSessionNotification::new(session_id)).await.expect("cancel queues");

            cancelled.notified().await;
            drop((prompt_responder, config_responder));
            client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_completion_follows_session_updates_on_the_event_stream() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent_builder = Agent
                .v2()
                .on_receive_request(
                    async |_request: InitializeRequest, responder, _cx| responder.respond(initialize_response()),
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |request: PromptRequest, responder, cx| {
                        cx.send_notification(UpdateSessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new("final answer")),
                                "answer",
                            )),
                        ))?;
                        cx.send_notification(acp_utils::testing::idle_notification(
                            request.session_id,
                            Some(StopReason::EndTurn),
                        ))?;
                        responder.respond(PromptResponse::new())
                    },
                    acp::on_receive_request!(),
                );
            spawn_local(async move {
                let _ = agent_builder.connect_to(agent_transport).await;
            });

            let mut client =
                connect_acp_client(client_transport, initialize_request()).await.expect("initialization succeeds");
            client
                .handle
                .prompt(PromptRequest::new("session", vec![ContentBlock::Text(TextContent::new("hello"))]))
                .await
                .expect("prompt succeeds");

            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::SessionUpdate { .. })));
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::SessionUpdate { .. })));
            assert!(matches!(
                client.event_rx.recv().await,
                Some(AcpEvent::PromptCompleted { stop_reason: StopReason::EndTurn, .. })
            ));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resumed_snapshot_is_delivered_on_the_event_channel() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let mut client = TestClientBuilder::default()
                .replay_message("saved", "snapshot")
                .compaction_active(true)
                .live_message("saved", "live")
                .build()
                .await?;

            client
                .handle
                .resume_session(
                    ResumeSessionRequest::new("saved", "/remote")
                        .replay_from(ReplayFrom::Start(ReplayFromStart::new())),
                )
                .await?;
            assert!(client.event_rx.try_recv().is_err(), "plain resume must request no history");
            client.handle.resume_session_with_replay(ResumeSessionRequest::new("saved", "/remote")).await?;

            let loaded = take_resumed_session(&mut client)?;
            assert_eq!(loaded.session_id, SessionId::new("saved"));
            let [
                ReplayableEvent::SessionUpdate(notification),
                ReplayableEvent::ContextCompaction(compaction),
                ReplayableEvent::SessionUpdate(idle),
            ] = loaded.replay.as_slice()
            else {
                return Err(TestError::Unexpected("expected message followed by compaction in replay"));
            };
            assert_eq!(notification.as_ref(), &message("saved", "snapshot"));
            assert!(compaction.active);
            assert_eq!(idle.as_ref(), &acp_utils::testing::idle_notification("saved", None));
            let Some(AcpEvent::SessionUpdate { session_id, update }) = client.event_rx.recv().await else {
                return Err(TestError::Unexpected("expected live update after snapshot"));
            };
            assert_eq!(session_id, SessionId::new("saved"));
            assert_eq!(*update, message("saved", "live").update);
            Ok(())
        })
        .await
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "current_thread")]
async fn initialized_client_manages_typed_sessions_and_collects_replay() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let agent_builder = TestClientBuilder::default()
                .agent_info(Implementation::new("Typed Fake", "1.0"))
                .replay_message("other", "unrelated")
                .replay_message("listed", "replayed")
                .agent()
                .on_receive_request(
                    async |_request: NewSessionRequest, responder, _cx| {
                        responder.respond(NewSessionResponse::new(SessionId::new("created")))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_request: ListSessionsRequest, responder, _cx| {
                        responder.respond(ListSessionsResponse::new(vec![SessionInfo::new("listed", "/tmp/project")]))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_request: CloseSessionRequest, responder, _cx| {
                        responder.respond(CloseSessionResponse::new())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_request: PromptSearchParams, responder, _cx| {
                        responder.respond(PromptSearchResponse {
                            query: "hello".to_string(),
                            results: vec![],
                            truncated: false,
                        })
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_request: SessionPreviewParams, responder, _cx| {
                        responder.respond(SessionPreviewResponse {
                            session_id: "listed".to_string(),
                            cwd: PathBuf::from("/tmp/project"),
                            created_at: "now".to_string(),
                            model: "fake".to_string(),
                            selected_mode: None,
                            transcript: vec![],
                            tool_call_count: 0,
                            truncated: false,
                        })
                    },
                    acp::on_receive_request!(),
                );
            let mut client = connect_test_agent(agent_builder).await?;
            assert_eq!(client.agent_name(), "Typed Fake");
            assert_eq!(client.initialize_response.info.name, "Typed Fake");

            let created =
                client.handle.new_session(NewSessionRequest::new("/tmp/project")).await?;
            assert_eq!(created.session_id, SessionId::new("created"));

            let listed = client.handle.list_sessions(ListSessionsRequest::new()).await?;
            assert_eq!(listed.sessions.len(), 1);
            assert_eq!(listed.sessions[0].session_id, SessionId::new("listed"));

            client.handle.resume_session_with_replay(ResumeSessionRequest::new("listed", "/tmp/project")).await?;
            let event = client.event_rx.try_recv()?;
            assert!(
                matches!(event, AcpEvent::SessionUpdate { session_id, .. } if session_id == SessionId::new("other"))
            );
            let loaded = take_resumed_session(&mut client)?;
            assert_eq!(loaded.replay.len(), 2);
            assert!(matches!(&loaded.replay[0], ReplayableEvent::SessionUpdate(notification) if notification.session_id == SessionId::new("listed")));

            client
                .handle
                .resume_session(ResumeSessionRequest::new("listed", "/tmp/project"))
                .await?;
            let search = client
                .handle
                .search_prompts(PromptSearchParams { query: "hello".to_string(), limit: Some(10) })
                .await?;
            assert_eq!(search.query, "hello");
            let preview = client
                .handle
                .preview_session(SessionPreviewParams { session_id: "listed".to_string() })
                .await?;
            assert_eq!(preview.session_id, "listed");
            client.handle.close_session(CloseSessionRequest::new("listed")).await?;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn initialization_without_capabilities_exposes_none() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let client = TestClientBuilder::default().build().await?;
            assert!(client.prompt_capabilities().is_none());
            assert!(client.session_capabilities().is_none());
            client.handle.disconnect().await;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn v2_initialization_accessors_expose_agent_metadata() -> Result<(), TestError> {
    use acp::schema::v2::{PromptCapabilities, SessionCapabilities};
    LocalSet::new()
        .run_until(async {
            let client = TestClientBuilder::default()
                .agent_info(Implementation::new("agent", "1").title("Display Name"))
                .capabilities(
                    AgentCapabilities::new().session(SessionCapabilities::new().prompt(PromptCapabilities::new())),
                )
                .build()
                .await?;
            assert_eq!(client.initialize_response.protocol_version, ProtocolVersion::V2);
            assert_eq!(client.agent_name(), "Display Name");
            assert!(client.prompt_capabilities().is_some());
            assert!(client.session_capabilities().is_some());
            assert!(client.auth_methods().is_empty());
            client.handle.disconnect().await;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn login_accepts_supported_methods_and_rejects_unknown_methods() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let client = TestClientBuilder::default().login_method("login").build().await?;
            assert!(client.handle.login(LoginAuthRequest::new("unknown")).await.is_err());
            client.handle.login(LoginAuthRequest::new("login")).await?;
            client.handle.disconnect().await;
            Ok(())
        })
        .await
}

#[derive(Default)]
struct TestClientBuilder {
    agent_info: Option<Implementation>,
    replay: Vec<UpdateSessionNotification>,
    live: Vec<UpdateSessionNotification>,
    compaction_active: Option<bool>,
    capabilities: Option<AgentCapabilities>,
    login_method: Option<&'static str>,
}

impl TestClientBuilder {
    fn agent_info(mut self, info: Implementation) -> Self {
        self.agent_info = Some(info);
        self
    }

    fn capabilities(mut self, capabilities: AgentCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    fn login_method(mut self, method: &'static str) -> Self {
        self.login_method = Some(method);
        self
    }

    fn replay_message(mut self, session_id: &str, text: &str) -> Self {
        self.replay.push(message(session_id, text));
        self
    }

    fn live_message(mut self, session_id: &str, text: &str) -> Self {
        self.live.push(message(session_id, text));
        self
    }

    fn compaction_active(mut self, active: bool) -> Self {
        self.compaction_active = Some(active);
        self
    }

    async fn build(self) -> Result<AcpClient, TestError> {
        connect_test_agent(self.agent()).await
    }

    fn agent(self) -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
        Agent
            .v2()
            .on_receive_request(
                async move |_: InitializeRequest, responder, _cx| {
                    let mut response = initialize_response();
                    if let Some(info) = &self.agent_info {
                        response.info = info.clone();
                    }
                    if let Some(capabilities) = &self.capabilities {
                        response = response.capabilities(capabilities.clone());
                    }
                    responder.respond(response)
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: LoginAuthRequest, responder, _cx| {
                    if self.login_method == Some(request.method_id.0.as_ref()) {
                        responder.respond(LoginAuthResponse::new())
                    } else {
                        responder.respond_with_error(acp::Error::invalid_params())
                    }
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ResumeSessionRequest, responder, cx| {
                    if request.replay_from.is_none() {
                        return responder.respond(ResumeSessionResponse::new());
                    }
                    for notification in &self.replay {
                        cx.send_notification(notification.clone())?;
                    }
                    if let Some(active) = self.compaction_active {
                        cx.send_notification(ContextCompactionParams { active })?;
                    }
                    cx.send_notification(idle_notification(request.session_id, None))?;
                    responder.respond(ResumeSessionResponse::new())?;
                    for notification in &self.live {
                        cx.send_notification(notification.clone())?;
                    }
                    Ok(())
                },
                acp::on_receive_request!(),
            )
    }
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error(transparent)]
    Receive(#[from] TryRecvError),
    #[error("{0}")]
    Unexpected(&'static str),
}

async fn connect_test_agent(agent: impl ConnectTo<Client> + 'static) -> Result<AcpClient, TestError> {
    let (agent_transport, client_transport) = duplex_pair();
    spawn_local(async move {
        let _ = agent.connect_to(agent_transport).await;
    });
    Ok(connect_acp_client(client_transport, acp_utils::testing::initialize_request()).await?)
}

fn take_resumed_session(client: &mut AcpClient) -> Result<ResumedSession, TestError> {
    let AcpEvent::SessionResumed(loaded) = client.event_rx.try_recv()? else {
        return Err(TestError::Unexpected("expected loaded snapshot before live events"));
    };
    Ok(loaded)
}

fn message(session_id: &str, text: &str) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        SessionId::new(session_id.to_owned()),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)), "message")),
    )
}
