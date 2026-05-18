//! Duplex-backed test harness for ACP connections.
//!
//! [`test_connection`] returns a full `(ConnectionTo<Client>, TestPeer)` pair
//! over an in-memory duplex transport. Use it for integration-style tests that
//! need to exercise the full serialize/dispatch path (so wire-format
//! regressions like extension method-name typos surface in tests).
//!
//! When a test needs to pass a real [`Responder<ElicitationResponse>`] into a
//! component under test (e.g. an elicitation UI) and observe what that
//! component eventually sends, call [`TestPeer::fake_elicitation`]: it kicks
//! off a placeholder elicitation request, hands back the captured responder,
//! and returns a receiver that resolves when the responder is consumed.

use crate::notifications::{ElicitationParams, ElicitationResponse, McpNotification};
use agent_client_protocol::schema::SessionNotification;
use agent_client_protocol::{
    self as acp, Agent, Builder, ByteStreams, Client, ConnectionTo, HandleDispatchFrom, NullRun, Responder,
};
use rmcp::model::{CreateElicitationRequestParams, ElicitationSchema};
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
    elicitation_requests: mpsc::UnboundedReceiver<ElicitationParams>,
    elicitation_responses: Arc<Mutex<VecDeque<ElicitationResponse>>>,
    responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<ElicitationResponse>>>>>,
}

impl TestPeer {
    /// Build a `TestPeer` plus a pre-wired `Client.builder()` whose
    /// notification handlers route session/mcp/elicitation traffic into the
    /// peer. The caller decides whether to run the builder via `connect_to`
    /// (drop the agent-side cx) or `connect_with` (capture the agent-side cx).
    pub fn new() -> (Self, Builder<Client, impl HandleDispatchFrom<Agent>, NullRun>) {
        let (sn_tx, sn_rx) = mpsc::unbounded_channel::<SessionNotification>();
        let (mcp_tx, mcp_rx) = mpsc::unbounded_channel::<McpNotification>();
        let (el_tx, el_rx) = mpsc::unbounded_channel::<ElicitationParams>();
        let elicitation_responses: Arc<Mutex<VecDeque<ElicitationResponse>>> = Arc::new(Mutex::new(VecDeque::new()));
        let responder_capture: Arc<Mutex<Option<oneshot::Sender<Responder<ElicitationResponse>>>>> =
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
            .on_receive_request(
                {
                    let tx = el_tx;
                    let responses = elicitation_responses.clone();
                    let capture = responder_capture.clone();
                    async move |req: ElicitationParams, responder: Responder<ElicitationResponse>, _cx| {
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

    pub async fn next_elicitation_request(&mut self) -> ElicitationParams {
        self.elicitation_requests.recv().await.expect("peer channel closed")
    }

    /// Queue a response the peer will hand back for the next incoming
    /// elicitation request. If the queue is empty when a request arrives, the
    /// peer responds with a protocol error, which exercises the
    /// `cancel_result()` fallback path in the caller.
    pub fn queue_elicitation_response(&self, response: ElicitationResponse) {
        self.elicitation_responses.lock().unwrap().push_back(response);
    }

    pub fn capture_next_elicitation(&self) -> oneshot::Receiver<Responder<ElicitationResponse>> {
        let (responder_tx, responder_rx) = oneshot::channel::<Responder<ElicitationResponse>>();
        *self.responder_capture.lock().unwrap() = Some(responder_tx);
        responder_rx
    }

    /// Kick off a placeholder elicitation request from the agent side of `cx`,
    /// hand back the [`Responder<ElicitationResponse>`] captured on the client
    /// side, and return a receiver that resolves when the responder is
    /// consumed.
    ///
    /// Use in tests that pass a `Responder<ElicitationResponse>` into code
    /// under test and want to observe the response without driving a full ACP
    /// round-trip themselves.
    pub async fn fake_elicitation(
        &mut self,
        cx: &ConnectionTo<Client>,
    ) -> (Responder<ElicitationResponse>, oneshot::Receiver<ElicitationResponse>) {
        let (responder_tx, responder_rx) = oneshot::channel::<Responder<ElicitationResponse>>();
        *self.responder_capture.lock().unwrap() = Some(responder_tx);

        let (response_tx, response_rx) = oneshot::channel::<ElicitationResponse>();
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

fn placeholder_params() -> ElicitationParams {
    ElicitationParams {
        server_name: String::new(),
        request: CreateElicitationRequestParams::FormElicitationParams {
            meta: None,
            message: String::new(),
            requested_schema: ElicitationSchema::builder().build().expect("empty schema is valid"),
        },
    }
}
