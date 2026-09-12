use acp::schema::v2::{
    CancelSessionNotification, ContentChunk, InitializeRequest, PromptRequest, PromptResponse, ResumeSessionRequest,
    ResumeSessionResponse, SessionId, SessionUpdate, StopReason, UpdateSessionNotification,
};
use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::testing::{
    duplex_pair, idle_notification, initialize_request, initialize_response, running_notification,
};
use agent_client_protocol::{self as acp, Agent, Client, ConnectionTo, Responder};
use std::collections::HashSet;
use tokio::sync::{mpsc, watch};
use tokio::task::{LocalSet, spawn_local};

#[tokio::test]
async fn cancellation_before_and_after_acceptance_waits_for_agent_idle() {
    LocalSet::new()
        .run_until(async {
            let mut fake = TurnTestBuilder::default().build().await;
            for before_ack in [true, false] {
                let prompt = fake.submit("one").await;
                if before_ack {
                    fake.cancel("one").await;
                    assert!(fake.client.event_rx.try_recv().is_err());
                }
                prompt.accept().await;
                if !before_ack {
                    fake.cancel("one").await;
                }
                assert!(fake.client.event_rx.try_recv().is_err());
                fake.send(UpdateSessionNotification::new(
                    "one",
                    SessionUpdate::AgentMessageChunk(ContentChunk::new("last chunk".into(), "answer")),
                ));
                fake.send(idle_notification("one", Some(StopReason::Cancelled)));
                fake.update().await;
                fake.update().await;
                fake.completed("one", StopReason::Cancelled).await;
            }
            fake.cancel("one").await;
            fake.send(idle_notification("one", None));
            fake.update().await;
            assert!(fake.client.event_rx.try_recv().is_err());
            fake.client.handle.disconnect().await;
        })
        .await;
}

#[tokio::test]
async fn one_foreground_turn_blocks_other_sessions_until_idle() {
    LocalSet::new()
        .run_until(async {
            let mut fake = TurnTestBuilder::default().build().await;
            for session in ["one", "two"] {
                let prompt = fake.submit(session).await;
                fake.assert_prompt_busy("other").await;
                for replay in [false, true] {
                    fake.assert_resume_busy("other", replay).await;
                }
                prompt.accept().await;
                fake.assert_prompt_busy("other").await;
                for replay in [false, true] {
                    fake.assert_resume_busy("other", replay).await;
                }
                fake.send(running_notification("background"));
                fake.send(idle_notification("background", None));
                fake.update().await;
                fake.update().await;
                assert!(fake.client.event_rx.try_recv().is_err());
                fake.assert_prompt_busy("other").await;
                fake.send(idle_notification(session, Some(StopReason::EndTurn)));
                fake.update().await;
                fake.completed(session, StopReason::EndTurn).await;
            }
            fake.client.handle.disconnect().await;
            assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
            assert!(fake.client.event_rx.recv().await.is_none());
        })
        .await;
}

#[derive(Default)]
struct TurnTestBuilder {
    accepted_turn: Option<&'static str>,
}

impl TurnTestBuilder {
    async fn build(self) -> TurnTest {
        let mut test = TurnTest::connect().await;
        if let Some(session) = self.accepted_turn {
            test.submit(session).await.accept().await;
        }
        test
    }
}

struct PendingPrompt {
    responder: Responder<PromptResponse>,
    result: tokio::task::JoinHandle<Result<PromptResponse, AcpClientError>>,
}

impl PendingPrompt {
    async fn accept(self) {
        self.responder.respond(PromptResponse::new()).unwrap();
        self.result.await.unwrap().unwrap();
    }
}

struct TurnTest {
    client: AcpClient,
    connection: ConnectionTo<Client>,
    prompts: mpsc::UnboundedReceiver<Responder<PromptResponse>>,
    cancelled_sessions: watch::Receiver<HashSet<SessionId>>,
    resumes: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
}

impl TurnTest {
    async fn connect() -> Self {
        let (agent_transport, client_transport) = duplex_pair();
        let (connections, mut connection_rx) = mpsc::unbounded_channel();
        let (prompts, prompt_rx) = mpsc::unbounded_channel();
        let (cancelled_sessions, cancel_rx) = watch::channel(HashSet::new());
        let prompt_sessions = cancelled_sessions.clone();
        let (resumes, resume_rx) = mpsc::unbounded_channel();
        let agent = Agent
            .v2()
            .on_receive_request(
                async move |_: InitializeRequest, responder, cx| {
                    connections.send(cx.clone()).unwrap();
                    responder.respond(initialize_response())
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, _cx| {
                    prompt_sessions.send_modify(|sessions| {
                        sessions.remove(&request.session_id);
                    });
                    prompts.send(responder).unwrap();
                    Ok(())
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ResumeSessionRequest, responder, _cx| {
                    resumes.send((request, responder)).unwrap();
                    Ok(())
                },
                acp::on_receive_request!(),
            )
            .on_receive_notification(
                async move |request: CancelSessionNotification, _cx| {
                    cancelled_sessions.send_modify(|sessions| {
                        sessions.insert(request.session_id);
                    });
                    Ok(())
                },
                acp::on_receive_notification!(),
            );
        spawn_local(agent.connect_to(agent_transport));
        let client = connect_acp_client(client_transport, initialize_request()).await.unwrap();
        Self {
            client,
            connection: connection_rx.recv().await.unwrap(),
            prompts: prompt_rx,
            cancelled_sessions: cancel_rx,
            resumes: resume_rx,
        }
    }

    async fn submit(&mut self, session: &str) -> PendingPrompt {
        let handle = self.client.handle.clone();
        let request = PromptRequest::new(session.to_owned(), vec![]);
        let result = spawn_local(async move { handle.prompt(request).await });
        PendingPrompt { responder: self.prompts.recv().await.unwrap(), result }
    }

    async fn cancel(&mut self, session: &str) {
        let session_id = SessionId::new(session.to_owned());
        self.client.handle.cancel(CancelSessionNotification::new(session_id.clone())).await.unwrap();
        let state = self.cancelled_sessions.wait_for(|sessions| sessions.contains(&session_id)).await.unwrap();
        assert!(state.contains(&session_id));
    }

    fn resume(
        &self,
        session: &str,
        replay: bool,
    ) -> tokio::task::JoinHandle<Result<ResumeSessionResponse, AcpClientError>> {
        let handle = self.client.handle.clone();
        let request = ResumeSessionRequest::new(session.to_owned(), "/tmp");
        spawn_local(async move {
            if replay { handle.resume_session_with_replay(request).await } else { handle.resume_session(request).await }
        })
    }

    async fn assert_prompt_busy(&mut self, session: &str) {
        let handle = self.client.handle.clone();
        let request = PromptRequest::new(session.to_owned(), vec![]);
        let mut task = spawn_local(async move { handle.prompt(request).await });
        let result = tokio::select! {
            result = &mut task => result.unwrap(),
            responder = self.prompts.recv() => {
                responder.unwrap().respond(PromptResponse::new()).unwrap();
                task.await.unwrap()
            }
        };
        assert!(matches!(result, Err(AcpClientError::Busy)), "expected Busy, got {result:?}");
    }

    async fn assert_resume_busy(&mut self, session: &str, replay: bool) {
        let mut task = self.resume(session, replay);
        let result = tokio::select! {
            result = &mut task => result.unwrap(),
            request = self.resumes.recv() => {
                request.unwrap().1.respond(ResumeSessionResponse::new()).unwrap();
                task.await.unwrap()
            }
        };
        assert!(matches!(result, Err(AcpClientError::Busy)), "expected Busy, got {result:?}");
    }

    fn send(&self, notification: UpdateSessionNotification) {
        self.connection.send_notification(notification).unwrap();
    }

    async fn update(&mut self) {
        assert!(matches!(self.client.event_rx.recv().await, Some(AcpEvent::SessionUpdate { .. })));
    }

    async fn completed(&mut self, session: &str, reason: StopReason) {
        assert!(
            matches!(self.client.event_rx.recv().await, Some(AcpEvent::PromptCompleted { session_id, stop_reason }) if session_id == SessionId::new(session.to_owned()) && stop_reason == reason)
        );
    }
}
