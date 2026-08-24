//! Duplex-backed test harness for ACP connections.
//!
//! [`test_connection`] returns a full `(ConnectionTo<Client>, TestPeer)` pair
//! over an in-memory duplex transport. Use it for integration-style tests that
//! need to exercise the full serialize/dispatch path (so wire-format
//! regressions like extension method-name typos surface in tests).
//!

use crate::notifications::McpNotification;
use agent_client_protocol::schema::v1::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, ElicitationFormMode,
    ElicitationSchema, ElicitationSessionScope, SessionNotification,
};
use agent_client_protocol::{
    self as acp, Agent, Builder, ByteStreams, Client, ConnectionTo, HandleDispatchFrom, NullRun, Responder,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::io::DuplexStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_local;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub type DuplexByteStreams = ByteStreams<Compat<DuplexStream>, Compat<DuplexStream>>;

pub struct TestPeer {
    session_notifications: mpsc::UnboundedReceiver<SessionNotification>,
    mcp_notifications: mpsc::UnboundedReceiver<McpNotification>,
    elicitation_requests: mpsc::UnboundedReceiver<CreateElicitationRequest>,
    elicitation_completions: mpsc::UnboundedReceiver<CompleteElicitationNotification>,
    elicitation_responses: Arc<Mutex<VecDeque<CreateElicitationResponse>>>,
    responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>>,
}

impl TestPeer {
    pub fn new() -> (Self, Builder<Client, impl HandleDispatchFrom<Agent>, NullRun>) {
        let (sn_tx, sn_rx) = mpsc::unbounded_channel::<SessionNotification>();
        let (mcp_tx, mcp_rx) = mpsc::unbounded_channel::<McpNotification>();
        let (el_tx, el_rx) = mpsc::unbounded_channel::<CreateElicitationRequest>();
        let (complete_tx, complete_rx) = mpsc::unbounded_channel::<CompleteElicitationNotification>();
        let elicitation_responses: Arc<Mutex<VecDeque<CreateElicitationResponse>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<CreateElicitationResponse>>>>> =
            Arc::new(Mutex::new(None));

        let builder = Client
            .builder()
            .on_receive_notification(
                {
                    let tx = sn_tx;
                    async move |n: SessionNotification, _cx| {
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

    pub async fn next_session_notification(&mut self) -> SessionNotification {
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
    let (agent_transport, client_transport) = duplex_pair();

    spawn_local(async move {
        let _ = client_builder.connect_to(client_transport).await;
    });

    let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Client>>();
    spawn_local(async move {
        let _ = Agent
            .builder()
            .connect_with(agent_transport, async move |cx: ConnectionTo<Client>| {
                let _ = cx_tx.send(cx);
                std::future::pending::<()>().await;
                Ok(())
            })
            .await;
    });

    let cx = cx_rx.await.expect("agent side connect_with produced a ConnectionTo");
    (cx, peer)
}

fn placeholder_params() -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        String::new(),
    )
}
