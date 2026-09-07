use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

pub(crate) struct CaptureServer {
    pub(crate) base_url: String,
    receiver: mpsc::UnboundedReceiver<CapturedRequest>,
    ws_receiver: mpsc::UnboundedReceiver<CapturedEnvelope>,
    ws: FakeResponsesWs,
    reject_ws_handshake: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResponseSpec {
    pub(crate) status: u16,
    pub(crate) body: String,
    pub(crate) headers: HashMap<String, String>,
}

impl ResponseSpec {
    pub(crate) fn sse(body: &str) -> Self {
        Self { status: 200, body: body.to_string(), headers: HashMap::new() }
    }

    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(name.to_string(), value.to_string());
        self
    }
}

struct CaptureState {
    sender: mpsc::UnboundedSender<CapturedRequest>,
    response: ResponseSpec,
    ws: FakeResponsesWs,
    ws_sender: mpsc::UnboundedSender<CapturedEnvelope>,
    reject_ws_handshake: Arc<AtomicBool>,
}

pub(crate) struct CapturedRequest {
    pub(crate) path: String,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Value,
}

/// One `response.create` envelope received over the fake WebSocket, together
/// with the handshake headers of its connection.
#[derive(Debug, Clone)]
pub(crate) struct CapturedEnvelope {
    pub(crate) headers: HeaderMap,
    pub(crate) body: Value,
}

/// How the fake WebSocket server answers the next `response.create` envelope.
#[derive(Debug, Clone)]
pub(crate) enum WsAction {
    /// Reply with a server `error` frame.
    Fail { status: u16, code: String, message: String },
    /// Drop the connection without responding (transport failure).
    DropConnection,
}

/// A handle to the fake Responses WebSocket: program behavior and observe
/// what the client actually sent.
#[derive(Clone, Default)]
struct FakeResponsesWs {
    actions: Arc<Mutex<VecDeque<WsAction>>>,
}

impl FakeResponsesWs {
    fn next_action(&self) -> Option<WsAction> {
        self.actions.lock().expect("ws fake lock").pop_front()
    }

    fn program(&self, actions: Vec<WsAction>) {
        *self.actions.lock().expect("ws fake lock") = actions.into();
    }
}

impl CaptureServer {
    pub(crate) async fn start_responses() -> Self {
        Self::start_with_response(RESPONSES_FIXTURE).await
    }

    pub(crate) async fn start_chat_completions() -> Self {
        Self::start_with_response(CHAT_COMPLETIONS_FIXTURE).await
    }

    pub(crate) async fn start_openrouter() -> Self {
        Self::start_with_response(OPENROUTER_FIXTURE).await
    }

    pub(crate) async fn start_with_response(response: &'static str) -> Self {
        Self::start_with_spec(ResponseSpec::sse(response)).await
    }

    pub(crate) async fn start_with_spec(spec: ResponseSpec) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (ws_sender, ws_receiver) = mpsc::unbounded_channel();
        let ws = FakeResponsesWs::default();
        let reject_ws_handshake = Arc::new(AtomicBool::new(false));
        let app = Router::new()
            .route("/responses", post(capture).get(ws_upgrade))
            .route("/chat/completions", post(capture))
            .route("/v1/chat/completions", post(capture))
            .with_state(Arc::new(CaptureState {
                sender,
                response: spec,
                ws: ws.clone(),
                ws_sender,
                reject_ws_handshake: reject_ws_handshake.clone(),
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { base_url, receiver, ws_receiver, ws, reject_ws_handshake }
    }

    pub(crate) async fn captured(&mut self) -> CapturedRequest {
        self.receiver.recv().await.expect("no request captured")
    }

    /// Await the next `response.create` envelope the client sends over WebSocket.
    pub(crate) async fn captured_ws(&mut self) -> CapturedEnvelope {
        self.ws_receiver.recv().await.expect("no WebSocket envelope captured")
    }

    /// Non-blocking check for WebSocket envelopes; `None` when the client
    /// never opened the WebSocket route.
    pub(crate) fn try_captured_ws(&mut self) -> Option<CapturedEnvelope> {
        self.ws_receiver.try_recv().ok()
    }

    /// Queue how the fake server answers upcoming `response.create` envelopes;
    /// unscripted envelopes get the default fixture completion.
    pub(crate) fn program_ws(&mut self, actions: Vec<WsAction>) {
        self.ws.program(actions);
    }

    /// Reject WebSocket upgrades with 401, as an endpoint does for an expired
    /// token; `allow_ws_handshake` turns the rejection back off.
    pub(crate) fn reject_ws_handshake(&self) {
        self.reject_ws_handshake.store(true, Ordering::SeqCst);
    }

    pub(crate) fn allow_ws_handshake(&self) {
        self.reject_ws_handshake.store(false, Ordering::SeqCst);
    }
}

const RESPONSES_FIXTURE: &str = include_str!("../../tests/fixtures/openai_responses/01_minimal.sse");
const CHAT_COMPLETIONS_FIXTURE: &str = include_str!("../../tests/fixtures/openai/01_minimal.sse");
const OPENROUTER_FIXTURE: &str = include_str!("../../tests/fixtures/openrouter/01_minimal.sse");

async fn capture(
    State(state): State<Arc<CaptureState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.sender.send(CapturedRequest { path: uri.path().to_string(), headers, body }).ok();
    let status = StatusCode::from_u16(state.response.status).unwrap_or(StatusCode::OK);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CONTENT_TYPE, "text/event-stream".parse().unwrap());
    for (name, value) in &state.response.headers {
        if let (Ok(name), Ok(value)) = (name.parse::<axum::http::HeaderName>(), value.parse()) {
            response_headers.insert(name, value);
        }
    }
    (status, response_headers, state.response.body.clone())
}

async fn ws_upgrade(
    State(state): State<Arc<CaptureState>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> axum::response::Response {
    if state.reject_ws_handshake.load(Ordering::SeqCst) {
        return (StatusCode::UNAUTHORIZED, "expired token").into_response();
    }
    upgrade.on_upgrade(move |socket| serve_ws_socket(state, headers, socket))
}

async fn serve_ws_socket(state: Arc<CaptureState>, handshake_headers: HeaderMap, mut socket: WebSocket) {
    while let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
        let envelope: Value = match serde_json::from_str(&text) {
            Ok(envelope) => envelope,
            Err(_) => continue,
        };
        let stream_id = envelope.get("stream_id").and_then(Value::as_str).map(String::from);
        state.ws_sender.send(CapturedEnvelope { headers: handshake_headers.clone(), body: envelope }).ok();

        match state.ws.next_action() {
            Some(WsAction::Fail { status, code, message }) => {
                let mut frame = json!({
                    "type": "error",
                    "status": status,
                    "error": {"code": code, "message": message}
                });
                if let Some(stream_id) = &stream_id {
                    frame["stream_id"] = json!(stream_id);
                }
                socket.send(AxumMessage::text(frame.to_string())).await.ok();
            }
            Some(WsAction::DropConnection) => return,
            None => {
                for frame in ws_fixture_frames(&state.response.body, stream_id.as_deref()) {
                    if socket.send(AxumMessage::text(frame)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Wrap a captured SSE body in server `message` frames, replaying each
/// fixture event over the WebSocket transport.
fn ws_fixture_frames(fixture: &str, stream_id: Option<&str>) -> Vec<String> {
    fixture
        .lines()
        .filter_map(|line| line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")))
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "[DONE]")
        .map(|data| {
            let event: Value = serde_json::from_str(data).expect("fixture data line is JSON");
            let mut frame = json!({"type": "message", "message": event});
            if let Some(stream_id) = stream_id {
                frame["stream_id"] = json!(stream_id);
            }
            frame.to_string()
        })
        .collect()
}
