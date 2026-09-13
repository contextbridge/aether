use acp::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, ContentChunk, ListSessionsRequest, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionUpdate, StopReason, UpdateSessionNotification,
};
use acp_utils::client::{AcpClient, AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::notifications::{
    ContextClearedParams, ContextCompactionParams, McpNotification, McpServerStatus, McpServerStatusEntry,
    SessionUsageParams, SubAgentEvent, SubAgentProgressParams,
};
use acp_utils::testing::{duplex_pair, idle_notification, initialize_request, running_notification};
use agent_client_protocol::{self as acp, Client, ConnectionTo, Responder};
use tokio::sync::mpsc;
use tokio::task::{LocalSet, spawn_local};

#[tokio::test]
async fn plain_resume_forwards_target_updates_without_collecting_history() {
    LocalSet::new().run_until(async {
        let mut fake = TurnTest::connect().await;
        let resume = fake.resume("one", false);
        let (request, responder) = fake.resumes.recv().await.unwrap();
        assert!(request.replay_from.is_none());
        fake.send(running_notification("inactive"));
        fake.send(running_notification("one"));
        assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(notification)) if notification.session_id == SessionId::new("one")));
        responder.respond(ResumeSessionResponse::new()).unwrap();
        resume.await.unwrap().unwrap();
        fake.client.handle.disconnect().await;
        assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
    }).await;
}

#[tokio::test]
async fn prompts_do_not_gate_requests_but_restorations_cannot_overlap() {
    LocalSet::new()
        .run_until(async {
            let mut fake = TurnTest::connect().await;
            let first = fake.submit("one").await;
            let second = fake.submit("one").await;
            fake.client.handle.request(CloseSessionRequest::new("saved")).await.unwrap();
            assert!(fake.client.handle.request(ListSessionsRequest::new()).await.unwrap().sessions.is_empty());
            let resume = fake.resume("two", true);
            let (_, responder) = fake.resumes.recv().await.unwrap();
            assert!(fake.client.handle.request(ListSessionsRequest::new()).await.unwrap().sessions.is_empty());
            for replay in [false, true] {
                fake.assert_resume_pending("other", replay).await;
            }
            responder.respond(ResumeSessionResponse::new()).unwrap();
            resume.await.unwrap().unwrap();
            assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::SessionResumed(_))));
            first.accept().await;
            second.accept().await;
            fake.client.handle.disconnect().await;
        })
        .await;
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
    prompts: mpsc::UnboundedReceiver<(PromptRequest, Responder<PromptResponse>)>,
    cancellations: mpsc::UnboundedReceiver<CancelSessionNotification>,
    resumes: mpsc::UnboundedReceiver<(ResumeSessionRequest, Responder<ResumeSessionResponse>)>,
    server: tokio::task::JoinHandle<Result<(), acp::Error>>,
}

impl TurnTest {
    async fn connect() -> Self {
        let (agent_transport, client_transport) = duplex_pair();
        let (agent, mut requests) = acp_utils::testing::FakeAgent::default().sessions(vec![]).capture();
        let server = spawn_local(agent.agent().connect_to(agent_transport));
        let client = connect_acp_client(client_transport, initialize_request()).await.unwrap();
        Self {
            client,
            connection: requests.connection.recv().await.unwrap(),
            prompts: requests.prompt,
            cancellations: requests.cancel,
            resumes: requests.resume,
            server,
        }
    }

    async fn permission(&self, session: &str) -> RequestPermissionOutcome {
        self.connection
            .send_request(RequestPermissionRequest::new(
                session.to_owned(),
                "Continue?",
                vec![PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce)],
            ))
            .block_task()
            .await
            .unwrap()
            .outcome
    }

    async fn submit(&mut self, session: &str) -> PendingPrompt {
        let handle = self.client.handle.clone();
        let request = PromptRequest::new(session.to_owned(), vec![]);
        let result = spawn_local(async move { handle.prompt(request).await });
        PendingPrompt { responder: self.prompts.recv().await.unwrap().1, result }
    }

    async fn cancel(&mut self, session: &str) {
        let session_id = SessionId::new(session.to_owned());
        self.client.handle.cancel(CancelSessionNotification::new(session_id.clone())).unwrap();
        assert_eq!(self.cancellations.recv().await.unwrap().session_id, session_id);
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

    async fn assert_resume_pending(&mut self, session: &str, replay: bool) {
        let result = self.resume(session, replay).await.unwrap();
        assert!(
            matches!(result, Err(AcpClientError::RestorationPending)),
            "expected pending restoration, got {result:?}"
        );
    }

    async fn assert_clean_retry(&mut self) {
        let retry = self.resume("one", true);
        let (_, responder) = self.resumes.recv().await.unwrap();
        let idle = idle_notification("one", None);
        self.send(idle.clone());
        responder.respond(ResumeSessionResponse::new()).unwrap();
        retry.await.unwrap().unwrap();
        let Some(AcpEvent::SessionResumed(snapshot)) = self.client.event_rx.recv().await else {
            panic!("retry must commit a fresh snapshot");
        };
        assert_eq!(snapshot.session_id, idle.session_id);
        assert!(matches!(snapshot.replay.as_slice(), [AcpEvent::SessionUpdate(update)] if **update == idle));
    }

    fn send(&self, notification: UpdateSessionNotification) {
        self.connection.send_notification(notification).unwrap();
    }

    async fn update(&mut self) {
        assert!(matches!(self.client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(_))));
    }

    async fn completed(&mut self, session: &str, reason: StopReason) {
        assert!(
            matches!(self.client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(notification)) if notification.session_id == SessionId::new(session.to_owned()) && notification.update == idle_notification(session.to_owned(), Some(reason)).update)
        );
    }
}
