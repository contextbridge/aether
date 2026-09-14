use acp::schema::v2::{AgentCapabilities, CompactionStatus, LoginAuthRequest};
use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse,
};
use acp_utils::testing::{FakeAgent, duplex_pair};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, ContentBlock, ContentChunk, Implementation, ListSessionsRequest,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ReplayFrom, ReplayFromStart,
    ResumeSessionRequest, SessionId, SessionInfo, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, TextContent, UpdateSessionNotification,
};
use agent_client_protocol::{self as acp, Client, ConnectTo};
use std::path::PathBuf;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::task::{LocalSet, spawn_local};

#[tokio::test(flavor = "current_thread")]
async fn cancel_reaches_the_agent_while_a_config_response_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = FakeAgent::default()
                .new_session_response(NewSessionResponse::new("sess-1"))
                .hold_config(true)
                .capture();
            let client = agent.build().await.expect("initialization succeeds");

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
            let (_, prompt_responder) = requests.prompt.recv().await.unwrap();
            let config_handle = client.handle.clone();
            let config_session_id = session_id.clone();
            spawn_local(async move {
                let _ =
                    config_handle.request(SetSessionConfigOptionRequest::new(config_session_id, "mode", "Plan")).await;
            });
            let config_responder = requests.pending_config.recv().await.unwrap();
            client.handle.cancel(CancelSessionNotification::new(session_id)).expect("cancel queues");

            assert_eq!(requests.cancel.recv().await.unwrap().session_id, SessionId::new("sess-1"));
            drop((prompt_responder, config_responder));
            client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_completion_follows_session_updates_on_the_event_stream() {
    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = FakeAgent::default().capture();
            let mut client = agent.build().await.expect("initialization succeeds");
            let cx = requests.connection.recv().await.unwrap();
            let prompt = client.handle.prompt(PromptRequest::new("session", vec![ContentBlock::from("hello")]));
            let (request, responder) = requests.prompt.recv().await.unwrap();
            cx.send_notification(UpdateSessionNotification::new(
                request.session_id.clone(),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("final answer"), "answer")),
            )).unwrap();
            cx.send_notification(acp_utils::testing::idle_notification(request.session_id, Some(StopReason::EndTurn))).unwrap();
            responder.respond(PromptResponse::new()).unwrap();
            prompt.await.expect("prompt succeeds");

            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(_))));
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(notification))
                if notification.update == acp_utils::testing::idle_notification("session", Some(StopReason::EndTurn)).update));
            client.handle.disconnect().await;
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn new_session_preserves_startup_updates_before_live_updates() {
    LocalSet::new()
        .run_until(async {
            let agent = FakeAgent::default().sessions(vec![]).agent().on_receive_request(
                async |_: NewSessionRequest, responder, cx| {
                    cx.send_notification(message("old", "stale"))?;
                    cx.send_notification(message("created", "startup"))?;
                    responder.respond(NewSessionResponse::new("created"))?;
                    cx.send_notification(message("created", "live"))
                },
                acp::on_receive_request!(),
            );
            let mut client = connect_test_agent(agent).await.unwrap();
            client.handle.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            client.handle.request(ListSessionsRequest::new()).await.unwrap();
            let mut updates = Vec::new();
            while let Ok(event) = client.event_rx.try_recv() {
                if let AcpEvent::SessionUpdate(notification) = event {
                    updates.push(*notification);
                }
            }
            assert_eq!(
                updates,
                vec![message("old", "stale"), message("created", "startup"), message("created", "live")]
            );
            client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn replay_updates_precede_the_resume_response() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let mut client = acp_utils::testing::FakeAgent::default()
                .replay_message("saved", "snapshot")
                .compaction("saved", "compaction", CompactionStatus::Completed)
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

            let replay = [client.event_rx.try_recv()?, client.event_rx.try_recv()?, client.event_rx.try_recv()?];
            let [
                AcpEvent::SessionUpdate(notification),
                AcpEvent::SessionUpdate(compaction),
                AcpEvent::SessionUpdate(idle),
            ] = replay.as_slice()
            else {
                return Err(TestError::Unexpected("expected message followed by compaction in replay"));
            };
            assert_eq!(notification.as_ref(), &message("saved", "snapshot"));
            assert!(matches!(&compaction.update, SessionUpdate::CompactionUpdate(update)
                if update.status == CompactionStatus::Completed));
            assert_eq!(idle.as_ref(), &acp_utils::testing::idle_notification("saved", None));
            let Some(AcpEvent::SessionUpdate(notification)) = client.event_rx.recv().await else {
                return Err(TestError::Unexpected("expected live update after snapshot"));
            };
            assert_eq!(notification.session_id, SessionId::new("saved"));
            assert_eq!(notification.update, message("saved", "live").update);
            Ok(())
        })
        .await
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "current_thread")]
async fn initialized_client_manages_typed_sessions_and_streams_replay() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let agent_builder = acp_utils::testing::FakeAgent::default()
                .agent_info(Implementation::new("Typed Fake", "1.0"))
                .replay_message("other", "unrelated")
                .replay_message("listed", "replayed")
                .new_session_response(NewSessionResponse::new("created"))
                .sessions(vec![SessionInfo::new("listed", "/tmp/project")])
                .agent()
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
            assert!(client.event_rx.try_recv().is_err());

            let listed = client.handle.request(ListSessionsRequest::new()).await?;
            assert_eq!(listed.sessions.len(), 1);
            assert_eq!(listed.sessions[0].session_id, SessionId::new("listed"));

            client.handle.resume_session_with_replay(ResumeSessionRequest::new("listed", "/tmp/project")).await?;
            for id in ["other", "listed", "listed"] {
                assert!(matches!(client.event_rx.try_recv()?, AcpEvent::SessionUpdate(notification) if notification.session_id == SessionId::new(id)));
            }

            client
                .handle
                .resume_session(ResumeSessionRequest::new("listed", "/tmp/project"))
                .await?;
            let search = client
                .handle
                .request(PromptSearchParams { query: "hello".to_string(), limit: Some(10) })
                .await?;
            assert_eq!(search.query, "hello");
            let preview = client
                .handle
                .request(SessionPreviewParams { session_id: "listed".to_string() })
                .await?;
            assert_eq!(preview.session_id, "listed");
            client.handle.request(CloseSessionRequest::new("listed")).await?;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn initialization_without_capabilities_exposes_none() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let client = acp_utils::testing::FakeAgent::default().build().await?;
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
            let client = acp_utils::testing::FakeAgent::default()
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
            let client = acp_utils::testing::FakeAgent::default().login_method("login").build().await?;
            assert!(client.handle.request(LoginAuthRequest::new("unknown")).await.is_err());
            client.handle.request(LoginAuthRequest::new("login")).await?;
            client.handle.disconnect().await;
            Ok(())
        })
        .await
}

#[tokio::test]
async fn permission_with_no_options_is_cancelled() {
    use acp::schema::v2::{RequestPermissionOutcome, RequestPermissionRequest};

    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = acp_utils::testing::FakeAgent::default().capture();
            let client = agent.build().await.unwrap();
            let connection = requests.connection.recv().await.unwrap();
            let prompt = client.handle.prompt(PromptRequest::new("session", vec![]));
            let (_, responder) = requests.prompt.recv().await.unwrap();
            let response = connection
                .send_request(RequestPermissionRequest::new("session", "Continue?", vec![]))
                .block_task()
                .await
                .unwrap();
            assert_eq!(response.outcome, RequestPermissionOutcome::Cancelled);
            responder.respond(PromptResponse::new()).unwrap();
            prompt.await.unwrap();
            client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test]
async fn requests_are_sent_in_call_order_without_polling_their_responses() {
    use futures::FutureExt;

    LocalSet::new()
        .run_until(async {
            let (agent, mut requests) = FakeAgent::default().sessions(vec![]).hold_config(true).capture();
            let client = agent.build().await.unwrap();
            let config = client.handle.request(SetSessionConfigOptionRequest::new("session", "mode", "Plan"));
            client.handle.request(ListSessionsRequest::new()).await.unwrap();
            let responder = requests
                .pending_config
                .recv()
                .now_or_never()
                .expect("config must already have been sent before the list request")
                .expect("config channel remains open");
            responder.respond(SetSessionConfigOptionResponse::new(vec![])).unwrap();
            config.await.unwrap();
            client.handle.disconnect().await;
        })
        .await;
}

#[test]
fn client_handles_are_send_sync_and_clone() {
    fn assert_traits<T: Send + Sync + Clone>() {}
    assert_traits::<acp_utils::client::AcpClientHandle>();
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

fn message(session_id: &str, text: &str) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        SessionId::new(session_id.to_owned()),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)), "message")),
    )
}
