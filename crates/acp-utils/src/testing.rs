//! Duplex-backed test harness for ACP connections.
//!
//! [`test_connection`] returns a full `(ConnectionTo<Client>, TestPeer)` pair
//! over an in-memory duplex transport. Use it for integration-style tests that
//! need to exercise the full serialize/dispatch path (so wire-format
//! regressions like extension method-name typos surface in tests).
//!

mod fake_agent;
pub use fake_agent::{FakeAgent, FakeAgentRequests};

use crate::notifications::McpNotification;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, ElicitationFormMode,
    ElicitationSchema, ElicitationSessionScope, IdleStateUpdate, Implementation, InitializeRequest, InitializeResponse,
    PlanEntry, PlanId, PlanUpdate, PlanUpdateContent, RunningStateUpdate, SessionId, SessionUpdate, StateUpdate,
    StopReason, UpdateSessionNotification,
};
use agent_client_protocol::{
    self as acp, Agent, ByteStreams, Client, ConnectionTo, HandleConnectionClose, HandleDispatchFrom, NullRun,
    Responder, RunWithConnectionTo, V2Builder,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::io::DuplexStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_local;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub type DuplexByteStreams = ByteStreams<Compat<DuplexStream>, Compat<DuplexStream>>;

pub struct TestPeer {
    session_notifications: mpsc::UnboundedReceiver<UpdateSessionNotification>,
    mcp_notifications: mpsc::UnboundedReceiver<McpNotification>,
    elicitation_requests: mpsc::UnboundedReceiver<CreateElicitationRequest>,
    elicitation_completions: mpsc::UnboundedReceiver<CompleteElicitationNotification>,
    elicitation_responses: Arc<Mutex<VecDeque<CreateElicitationResponse>>>,
    responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>>,
}

impl TestPeer {
    pub fn new() -> (Self, V2Builder<Client, impl HandleDispatchFrom<Agent>, NullRun>) {
        let (sn_tx, sn_rx) = mpsc::unbounded_channel::<UpdateSessionNotification>();
        let (mcp_tx, mcp_rx) = mpsc::unbounded_channel::<McpNotification>();
        let (el_tx, el_rx) = mpsc::unbounded_channel::<CreateElicitationRequest>();
        let (complete_tx, complete_rx) = mpsc::unbounded_channel::<CompleteElicitationNotification>();
        let elicitation_responses: Arc<Mutex<VecDeque<CreateElicitationResponse>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>> =
            Arc::new(Mutex::new(None));

        let builder = Client
            .v2()
            .name("test-client")
            .on_receive_notification(
                {
                    let tx = sn_tx;
                    async move |n: UpdateSessionNotification, _cx| {
                        let _ = tx.send(n);
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let tx = mcp_tx;
                    async move |n: McpNotification, _cx| {
                        let _ = tx.send(n);
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let tx = complete_tx;
                    async move |notification: CompleteElicitationNotification, _cx| {
                        let _ = tx.send(notification);
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let tx = el_tx;
                    let responses = elicitation_responses.clone();
                    let capture = responder_capture.clone();
                    async move |req: CreateElicitationRequest, responder: Responder<CreateElicitationResponse>, _cx| {
                        if let Some(capture_tx) = capture.lock().unwrap().take() {
                            return match capture_tx.send(responder) {
                                Ok(()) => Ok(()),
                                Err(responder) => responder.respond_with_error(acp::Error::internal_error()),
                            };
                        }
                        let _ = tx.send(req);
                        let queued = responses.lock().unwrap().pop_front();
                        match queued {
                            Some(response) => responder.respond(response),
                            None => responder.respond_with_error(acp::Error::method_not_found()),
                        }
                    }
                },
                acp::on_receive_request!(),
            );

        let peer = Self {
            session_notifications: sn_rx,
            mcp_notifications: mcp_rx,
            elicitation_requests: el_rx,
            elicitation_completions: complete_rx,
            elicitation_responses,
            responder_capture,
        };
        (peer, builder)
    }

    pub async fn next_session_notification(&mut self) -> UpdateSessionNotification {
        self.session_notifications.recv().await.expect("peer channel closed")
    }

    pub async fn next_mcp_notification(&mut self) -> McpNotification {
        self.mcp_notifications.recv().await.expect("peer channel closed")
    }

    pub async fn next_elicitation_request(&mut self) -> CreateElicitationRequest {
        self.elicitation_requests.recv().await.expect("peer channel closed")
    }

    pub async fn next_elicitation_completion(&mut self) -> CompleteElicitationNotification {
        self.elicitation_completions.recv().await.expect("peer channel closed")
    }

    pub fn queue_elicitation_response(&self, response: CreateElicitationResponse) {
        self.elicitation_responses.lock().unwrap().push_back(response);
    }

    pub async fn fake_elicitation(
        &mut self,
        cx: &ConnectionTo<Client>,
    ) -> (Responder<CreateElicitationResponse>, oneshot::Receiver<CreateElicitationResponse>) {
        let (responder_tx, responder_rx) = oneshot::channel::<Responder<CreateElicitationResponse>>();
        *self.responder_capture.lock().unwrap() = Some(responder_tx);

        let (response_tx, response_rx) = oneshot::channel::<CreateElicitationResponse>();
        let cx = cx.clone();
        spawn_local(async move {
            if let Ok(resp) = cx.send_request(placeholder_params()).block_task().await {
                let _ = response_tx.send(resp);
            }
        });

        let responder = responder_rx.await.expect("client handler must capture responder");
        (responder, response_rx)
    }
}

/// In-memory ACP transport pair: `(agent_transport, client_transport)`. Hand
/// each half to a `connect_to` / `connect_with` call on the corresponding
/// side. Must be used inside a `LocalSet` since the runners are `spawn_local`'d.
pub fn duplex_pair() -> (DuplexByteStreams, DuplexByteStreams) {
    let (agent_writer, client_reader) = tokio::io::duplex(4096);
    let (client_writer, agent_reader) = tokio::io::duplex(4096);
    let agent_transport = ByteStreams::new(agent_writer.compat_write(), agent_reader.compat());
    let client_transport = ByteStreams::new(client_writer.compat_write(), client_reader.compat());
    (agent_transport, client_transport)
}

/// Build a live `ConnectionTo<Client>` over an in-memory duplex transport with
/// a peer on the other end. Must be called inside a `LocalSet`.
pub async fn test_connection() -> (ConnectionTo<Client>, TestPeer) {
    let (peer, client_builder) = TestPeer::new();
    let agent = Agent.v2().name("test-agent").on_receive_request(
        async |_: InitializeRequest, responder: Responder<InitializeResponse>, _cx| {
            responder.respond(initialize_response())
        },
        acp::on_receive_request!(),
    );
    let pair = connect_pair(agent, client_builder).await;
    pair.client.send_request(initialize_request()).block_task().await.expect("initialize test peers");
    (pair.agent, peer)
}

pub struct ConnectedPair {
    pub agent: ConnectionTo<Client>,
    pub client: ConnectionTo<Agent>,
    pub agent_task: tokio::task::JoinHandle<Result<(), acp::Error>>,
    pub client_task: tokio::task::JoinHandle<Result<(), acp::Error>>,
}

pub async fn connect_pair<T, U, V, X, Y, Z>(
    agent: V2Builder<Agent, T, U, V>,
    client: V2Builder<Client, X, Y, Z>,
) -> ConnectedPair
where
    T: HandleDispatchFrom<Client> + 'static,
    U: RunWithConnectionTo<Client> + 'static,
    V: HandleConnectionClose<Client> + 'static,
    X: HandleDispatchFrom<Agent> + 'static,
    Y: RunWithConnectionTo<Agent> + 'static,
    Z: HandleConnectionClose<Agent> + 'static,
{
    let (agent_transport, client_transport) = duplex_pair();
    let (agent_tx, agent_rx) = oneshot::channel();
    let (client_tx, client_rx) = oneshot::channel();
    let agent_task =
        spawn_local(async move { agent.with_runner(CaptureConnection(agent_tx)).connect_to(agent_transport).await });
    let client_task =
        spawn_local(async move { client.with_runner(CaptureConnection(client_tx)).connect_to(client_transport).await });
    ConnectedPair {
        agent: agent_rx.await.expect("agent connection"),
        client: client_rx.await.expect("client connection"),
        agent_task,
        client_task,
    }
}

struct CaptureConnection<R: acp::Role>(oneshot::Sender<ConnectionTo<R>>);

impl<R: acp::Role> RunWithConnectionTo<R> for CaptureConnection<R> {
    async fn run_with_connection_to(self, cx: ConnectionTo<R>) -> Result<(), acp::Error> {
        let _ = self.0.send(cx.clone());
        cx.incoming_closed().await;
        Ok(())
    }
}

/// Initialization request from an in-memory v2 client.
pub fn initialize_request() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test-client", "0.0.0"))
}

/// Initialization response from an in-memory v2 agent.
pub fn initialize_response() -> InitializeResponse {
    InitializeResponse::new(ProtocolVersion::V2, Implementation::new("test-agent", "0.0.0"))
}

/// A live foreground turn has started.
pub fn running_notification(session_id: impl Into<SessionId>) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::StateUpdate(StateUpdate::Running(RunningStateUpdate::new())),
    )
}

/// Foreground work is idle, optionally with a reported stop reason.
pub fn idle_notification(
    session_id: impl Into<SessionId>,
    stop_reason: Option<StopReason>,
) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::StateUpdate(StateUpdate::Idle(IdleStateUpdate::new().stop_reason(stop_reason))),
    )
}

/// Replace the entries of an agent-owned plan.
pub fn plan_notification(
    session_id: impl Into<SessionId>,
    plan_id: impl Into<PlanId>,
    entries: Vec<PlanEntry>,
) -> UpdateSessionNotification {
    UpdateSessionNotification::new(
        session_id,
        SessionUpdate::PlanUpdate(PlanUpdate::new(PlanUpdateContent::items(plan_id, entries))),
    )
}

fn placeholder_params() -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        String::new(),
    )
}
