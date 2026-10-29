use acp::schema::v2::{
    CloseSessionRequest, ListSessionsRequest, PromptRequest, PromptResponse, ReplayFrom, ReplayFromStart,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, UpdateSessionNotification,
};
use acp_utils::client::{AcpClient, AcpClientError, AcpEvent};
use acp_utils::testing::{FakeAgent, FakeAgentRequests, running_notification};
use agent_client_protocol::{self as acp, Client, ConnectionTo, Responder};
use tokio::task::{LocalSet, spawn_local};

#[tokio::test]
async fn plain_resume_forwards_target_updates_without_collecting_history() {
    LocalSet::new().run_until(async {
        let mut fake = TurnTest::connect().await;
        let resume = fake.resume("one", false);
        let (request, responder) = fake.requests.held::<ResumeSessionRequest>().recv().await.unwrap();
        assert!(request.replay_from.is_none());
        fake.send(running_notification("one"));
        assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::SessionUpdate(notification)) if notification.session_id == SessionId::new("one")));
        responder.respond(ResumeSessionResponse::new()).unwrap();
        resume.await.unwrap().unwrap();
        fake.client.handle.disconnect().await;
        assert!(matches!(fake.client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
    }).await;
}

#[tokio::test]
async fn prompts_and_resumes_do_not_gate_requests() {
    LocalSet::new()
        .run_until(async {
            let mut fake = TurnTest::connect().await;
            let first = fake.submit("one").await;
            let second = fake.submit("one").await;
            fake.client.handle.request(CloseSessionRequest::new("saved")).await.unwrap();
            assert_eq!(fake.client.handle.request(ListSessionsRequest::new()).await.unwrap().sessions.len(), 0);
            let resume = fake.resume("two", true);
            let (_, responder) = fake.requests.held::<ResumeSessionRequest>().recv().await.unwrap();
            assert_eq!(fake.client.handle.request(ListSessionsRequest::new()).await.unwrap().sessions.len(), 0);
            for replay in [false, true] {
                let other = fake.resume("other", replay);
                let (_, responder) = fake.requests.held::<ResumeSessionRequest>().recv().await.unwrap();
                responder.respond(ResumeSessionResponse::new()).unwrap();
                other.await.unwrap().unwrap();
            }
            responder.respond(ResumeSessionResponse::new()).unwrap();
            resume.await.unwrap().unwrap();
            assert!(fake.client.event_rx.try_recv().is_err());
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
        self.responder.respond(PromptResponse::new("user-message")).unwrap();
        self.result.await.unwrap().unwrap();
    }
}

struct TurnTest {
    client: AcpClient,
    connection: ConnectionTo<Client>,
    requests: FakeAgentRequests,
}

impl TurnTest {
    async fn connect() -> Self {
        let (agent, mut requests) =
            FakeAgent::default().sessions(vec![]).hold::<PromptRequest>().hold::<ResumeSessionRequest>().capture();
        let client = agent.build().await.unwrap();
        Self { client, connection: requests.connection.recv().await.unwrap(), requests }
    }

    async fn submit(&mut self, session: &str) -> PendingPrompt {
        let handle = self.client.handle.clone();
        let request = PromptRequest::new(session.to_owned(), vec![]);
        let result = spawn_local(async move { handle.request(request).await });
        PendingPrompt { responder: self.requests.held::<PromptRequest>().recv().await.unwrap().1, result }
    }

    fn resume(
        &self,
        session: &str,
        replay: bool,
    ) -> tokio::task::JoinHandle<Result<ResumeSessionResponse, AcpClientError>> {
        let handle = self.client.handle.clone();
        let request = ResumeSessionRequest::new(session.to_owned(), "/tmp")
            .replay_from(replay.then(|| ReplayFrom::Start(ReplayFromStart::new())));
        spawn_local(async move { handle.request(request).await })
    }

    fn send(&self, notification: UpdateSessionNotification) {
        self.connection.send_notification(notification).unwrap();
    }
}
