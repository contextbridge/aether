use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, LoadedSession, ReplayableEvent, connect_acp_client};
use acp_utils::notifications::{
    ContextCompactionParams, PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse,
};
use acp_utils::testing::duplex_pair;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest,
    ResumeSessionResponse, SessionId, SessionInfo, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    StopReason, TextContent,
};
use agent_client_protocol::{self as acp, Agent, Builder, Client, ConnectTo, HandleDispatchFrom, NullRun};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc::error::TryRecvError};
use tokio::task::{LocalSet, spawn_local};

#[tokio::test(flavor = "current_thread")]
async fn cancel_reaches_the_agent_while_a_config_response_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let cancelled = Arc::new(Notify::new());

            let agent_builder = Agent
                .builder()
                .on_receive_request(
                    async |_req: InitializeRequest, responder, _cx| {
                        responder.respond(
                            InitializeResponse::new(ProtocolVersion::V1)
                                .agent_info(Implementation::new("Fake Agent", "0.0.0")),
                        )
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
                    async |_req: PromptRequest, responder, _cx| {
                        std::mem::forget(responder);
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_req: SetSessionConfigOptionRequest, responder, _cx| {
                        std::mem::forget(responder);
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_notification(
                    {
                        let cancelled = Arc::clone(&cancelled);
                        async move |_n: CancelNotification, _cx| {
                            cancelled.notify_one();
                            Ok(())
                        }
                    },
                    acp::on_receive_notification!(),
                );
            spawn_local(async move {
                let _ = agent_builder.connect_to(agent_transport).await;
            });

            let client = connect_acp_client(client_transport, InitializeRequest::new(ProtocolVersion::V1))
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
            let config_handle = client.handle.clone();
            let config_session_id = session_id.clone();
            spawn_local(async move {
                let _ = config_handle
                    .set_config_option(SetSessionConfigOptionRequest::new(config_session_id, "mode", "Plan"))
                    .await;
            });
            client.handle.cancel(CancelNotification::new(session_id)).await.expect("cancel queues");

            cancelled.notified().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_completion_follows_session_updates_on_the_event_stream() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent_builder = Agent
                .builder()
                .on_receive_request(
                    async |_request: InitializeRequest, responder, _cx| {
                        responder.respond(InitializeResponse::new(ProtocolVersion::V1))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |request: PromptRequest, responder, cx| {
                        cx.send_notification(SessionNotification::new(
                            request.session_id,
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
                                "final answer",
                            )))),
                        ))?;
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    },
                    acp::on_receive_request!(),
                );
            spawn_local(async move {
                let _ = agent_builder.connect_to(agent_transport).await;
            });

            let mut client = connect_acp_client(client_transport, InitializeRequest::new(ProtocolVersion::V1))
                .await
                .expect("initialization succeeds");
            client
                .handle
                .prompt(PromptRequest::new("session", vec![ContentBlock::Text(TextContent::new("hello"))]))
                .await
                .expect("prompt succeeds");

            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::SessionUpdate { .. })));
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::PromptCompleted(StopReason::EndTurn))));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn loaded_snapshot_is_delivered_on_the_event_channel() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let mut client = TestClientBuilder::default()
                .replay_message("saved", "snapshot")
                .compaction_active(true)
                .live_message("saved", "live")
                .build()
                .await?;

            client.handle.load_session(LoadSessionRequest::new("saved", "/remote")).await?;

            let loaded = take_loaded_session(&mut client)?;
            assert_eq!(loaded.session_id, SessionId::new("saved"));
            let [ReplayableEvent::SessionUpdate(notification), ReplayableEvent::ContextCompaction(compaction)] =
                loaded.replay.as_slice()
            else {
                return Err(TestError::Unexpected("expected message followed by compaction in replay"));
            };
            assert_eq!(notification.as_ref(), &message("saved", "snapshot"));
            assert!(compaction.active);
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
                    async |_request: ResumeSessionRequest, responder, _cx| {
                        responder.respond(ResumeSessionResponse::new())
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
            assert!(client.initialize_response.agent_info.is_some());

            let created =
                client.handle.new_session(NewSessionRequest::new("/tmp/project")).await?;
            assert_eq!(created.session_id, SessionId::new("created"));

            let listed = client.handle.list_sessions(ListSessionsRequest::new()).await?;
            assert_eq!(listed.sessions.len(), 1);
            assert_eq!(listed.sessions[0].session_id, SessionId::new("listed"));

            client.handle.load_session(LoadSessionRequest::new("listed", "/tmp/project")).await?;
            let event = client.event_rx.try_recv()?;
            assert!(
                matches!(event, AcpEvent::SessionUpdate { session_id, .. } if session_id == SessionId::new("other"))
            );
            let loaded = take_loaded_session(&mut client)?;
            assert_eq!(loaded.replay.len(), 1);
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

#[derive(Default)]
struct TestClientBuilder {
    agent_info: Option<Implementation>,
    replay: Vec<SessionNotification>,
    live: Vec<SessionNotification>,
    compaction_active: Option<bool>,
}

impl TestClientBuilder {
    fn agent_info(mut self, info: Implementation) -> Self {
        self.agent_info = Some(info);
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
            .builder()
            .on_receive_request(
                async move |_: InitializeRequest, responder, _cx| {
                    let mut response = InitializeResponse::new(ProtocolVersion::V1);
                    response.agent_info.clone_from(&self.agent_info);
                    responder.respond(response)
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async move |_: LoadSessionRequest, responder, cx| {
                    for notification in &self.replay {
                        cx.send_notification(notification.clone())?;
                    }
                    if let Some(active) = self.compaction_active {
                        cx.send_notification(ContextCompactionParams { active })?;
                    }
                    responder.respond(LoadSessionResponse::new())?;
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
    Ok(connect_acp_client(client_transport, InitializeRequest::new(ProtocolVersion::V1)).await?)
}

fn take_loaded_session(client: &mut AcpClient) -> Result<LoadedSession, TestError> {
    let AcpEvent::SessionLoaded(loaded) = client.event_rx.try_recv()? else {
        return Err(TestError::Unexpected("expected loaded snapshot before live events"));
    };
    Ok(loaded)
}

fn message(session_id: &str, text: &str) -> SessionNotification {
    SessionNotification::new(
        SessionId::new(session_id.to_owned()),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)))),
    )
}
