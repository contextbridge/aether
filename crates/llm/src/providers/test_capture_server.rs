use super::generic::{Api, GenericProvider, ProviderConfig};
use crate::{
    ChatMessage, Context, LlmResponse, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, Result,
    StreamingModelProvider,
};
use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

pub(crate) struct CaptureServer {
    pub(crate) base_url: String,
    pub(crate) upgrades: Arc<AtomicUsize>,
    receiver: mpsc::UnboundedReceiver<CapturedRequest>,
}

pub(crate) struct ProviderFixture {
    pub(crate) provider: GenericProvider,
    pub(crate) server: CaptureServer,
}

pub(crate) struct ProviderFixtureBuilder {
    config: &'static ProviderConfig,
    model: Option<String>,
    api_key: String,
    connection: ProviderConnectionConfig,
    response: ResponseSpec,
    sockets: SocketBehavior,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SocketBehavior {
    #[default]
    Remembers,
    CallsTool,
    SplitsReply,
    UnknownOutput,
    Forgets,
    Expires,
    ClosesWhenIdle,
    DropsMidResponse,
    RejectsUpgrade(StatusCode),
}

impl ProviderFixture {
    pub(crate) fn builder(config: &'static ProviderConfig) -> ProviderFixtureBuilder {
        let body = match config.api {
            Api::ChatCompletions(_) => CHAT_COMPLETIONS_FIXTURE,
            Api::Responses(..) => RESPONSES_FIXTURE,
            Api::Messages => MESSAGES_FIXTURE,
        };
        ProviderFixtureBuilder {
            config,
            model: None,
            api_key: "key".into(),
            connection: ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() },
            response: ResponseSpec::sse(body),
            sockets: SocketBehavior::default(),
        }
    }

    pub(crate) async fn run(&mut self, context: &Context) -> (CapturedRequest, Vec<Result<LlmResponse>>) {
        let events = self.provider.stream_response(context).collect::<Vec<_>>().await;
        let request = self
            .server
            .receiver
            .try_recv()
            .unwrap_or_else(|error| panic!("no request captured: {error}; events: {events:?}"));
        (request, events)
    }
}

impl ProviderFixtureBuilder {
    pub(crate) fn model(mut self, model: &str) -> Self {
        self.model = Some(model.into());
        self
    }

    pub(crate) fn api_key(mut self, api_key: &str) -> Self {
        self.api_key = api_key.into();
        self
    }

    pub(crate) fn connection(mut self, connection: ProviderConnectionConfig) -> Self {
        self.connection = connection;
        self
    }

    pub(crate) fn response(mut self, response: ResponseSpec) -> Self {
        self.response = response;
        self
    }

    pub(crate) fn sockets(mut self, sockets: SocketBehavior) -> Self {
        self.sockets = sockets;
        self
    }

    pub(crate) async fn build(mut self) -> ProviderFixture {
        let server = CaptureServer::start(self.response, self.sockets).await;
        self.connection.base_url.get_or_insert_with(|| format!("{}/", server.base_url));
        let provider = GenericProvider::new_with_connection(&self.api_key, self.config, self.connection).unwrap();
        let provider = match self.model {
            Some(model) => provider.with_model(&model),
            None => provider,
        };
        ProviderFixture { provider, server }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResponseSpec {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    pub(crate) headers: HashMap<String, String>,
    gap: Option<Duration>,
    interrupted: bool,
}

impl ResponseSpec {
    pub(crate) fn sse(body: &str) -> Self {
        Self { status: 200, body: body.into(), headers: HashMap::new(), gap: None, interrupted: false }
    }

    /// A binary AWS event stream, as Bedrock's native streaming APIs return.
    #[cfg(feature = "bedrock")]
    pub(crate) fn event_stream(body: Vec<u8>) -> Self {
        Self { body, ..Self::sse("") }.with_header("content-type", "application/vnd.amazon.eventstream")
    }

    pub(crate) fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(name.to_string(), value.to_string());
        self
    }

    pub(crate) fn paced(mut self, gap: Duration) -> Self {
        self.gap = Some(gap);
        self
    }

    /// Drop the connection after sending the response headers.
    pub(crate) fn interrupted(mut self) -> Self {
        self.interrupted = true;
        self
    }

    fn response_body(&self) -> Body {
        if self.interrupted {
            // Yielding first lets the server flush the headers before the body fails.
            return Body::from_stream(stream::once(async {
                tokio::task::yield_now().await;
                Err::<String, _>(std::io::Error::other("connection reset"))
            }));
        }
        let Some(gap) = self.gap else {
            return Body::from(self.body.clone());
        };
        let sse = String::from_utf8(self.body.clone()).expect("paced responses are server-sent events");
        let events: Vec<String> = sse.split_inclusive("\n\n").map(str::to_string).collect();
        Body::from_stream(stream::iter(events).then(move |event| async move {
            tokio::time::sleep(gap).await;
            Ok::<_, Infallible>(event)
        }))
    }
}

pub(crate) fn hello_context() -> Context {
    Context::new(vec![ChatMessage::user("Hello")], vec![])
}

#[derive(Debug)]
pub(crate) struct CapturedRequest {
    pub(crate) socket: Option<usize>,
    pub(crate) path: String,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Value,
}

impl CaptureServer {
    #[cfg(any(feature = "bedrock", feature = "codex"))]
    pub(crate) async fn start_responses() -> Self {
        Self::start_with_response(RESPONSES_FIXTURE).await
    }

    pub(crate) async fn start_chat_completions() -> Self {
        Self::start_with_response(CHAT_COMPLETIONS_FIXTURE).await
    }

    pub(crate) async fn start_with_response(response: &'static str) -> Self {
        Self::start_with_spec(ResponseSpec::sse(response)).await
    }

    pub(crate) async fn start_with_spec(spec: ResponseSpec) -> Self {
        Self::start(spec, SocketBehavior::default()).await
    }

    #[cfg(feature = "codex")]
    pub(crate) async fn start_responses_with_sockets(sockets: SocketBehavior) -> Self {
        Self::start(ResponseSpec::sse(RESPONSES_FIXTURE), sockets).await
    }

    async fn start(spec: ResponseSpec, sockets: SocketBehavior) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let upgrades = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/responses", post(capture).get(upgrade))
            .route("/v1/messages", post(capture))
            .route("/chat/completions", post(capture))
            .route("/v1/chat/completions", post(capture))
            .route("/model/{model}/converse-stream", post(capture))
            .route("/model/{model}/invoke-with-response-stream", post(capture))
            .with_state(Arc::new(CaptureState { sender, response: spec, sockets, opened: Arc::clone(&upgrades) }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { base_url, upgrades, receiver }
    }

    pub(crate) async fn captured(&mut self) -> CapturedRequest {
        self.receiver.recv().await.expect("no request captured")
    }

    pub(crate) fn drain(&mut self) -> Vec<CapturedRequest> {
        std::iter::from_fn(|| self.receiver.try_recv().ok()).collect()
    }

    pub(crate) async fn collect_on_paused_clock(&mut self, responses: LlmResponseStream) -> Vec<Result<LlmResponse>> {
        let (responses, ()) = tokio::join!(responses.collect::<Vec<_>>(), async {
            self.captured().await;
            tokio::time::pause();
        });
        tokio::time::resume();
        responses
    }
}

const RESPONSES_FIXTURE: &str = include_str!("../../tests/fixtures/openai_responses/01_minimal.sse");
const CHAT_COMPLETIONS_FIXTURE: &str = include_str!("../../tests/fixtures/openai/01_minimal.sse");
const MESSAGES_FIXTURE: &str = include_str!("../../tests/fixtures/anthropic/01_minimal.sse");

struct CaptureState {
    sender: mpsc::UnboundedSender<CapturedRequest>,
    response: ResponseSpec,
    sockets: SocketBehavior,
    opened: Arc<AtomicUsize>,
}

async fn capture(
    State(state): State<Arc<CaptureState>>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.sender.send(CapturedRequest { socket: None, path: uri.path().to_string(), headers, body }).ok();
    let status = StatusCode::from_u16(state.response.status).unwrap_or(StatusCode::OK);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CONTENT_TYPE, "text/event-stream".parse().unwrap());
    for (name, value) in &state.response.headers {
        if let (Ok(name), Ok(value)) = (name.parse::<axum::http::HeaderName>(), value.parse()) {
            response_headers.insert(name, value);
        }
    }
    (status, response_headers, state.response.response_body())
}

async fn upgrade(State(state): State<Arc<CaptureState>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let socket = state.opened.fetch_add(1, Ordering::Relaxed);
    if let SocketBehavior::RejectsUpgrade(status) = state.sockets {
        return status.into_response();
    }
    upgrade.on_upgrade(move |connection| serve_socket(connection, state, headers, socket))
}

async fn serve_socket(mut connection: WebSocket, state: Arc<CaptureState>, headers: HeaderMap, socket: usize) {
    let mut cached = None;
    for turn in 0.. {
        let Some(Ok(Message::Text(frame))) = connection.recv().await else { return };
        let body: Value = serde_json::from_str(&frame).expect("frames are JSON");
        let previous = body["previous_response_id"].as_str().map(str::to_owned);
        let captured =
            CapturedRequest { socket: Some(socket), path: "/responses".into(), headers: headers.clone(), body };
        state.sender.send(captured).ok();

        let id = format!("resp_{socket}_{turn}");
        let refusal = match state.sockets {
            SocketBehavior::Expires if turn > 0 => Some("websocket_connection_limit_reached"),
            _ if previous.is_some() && previous != cached => Some("previous_response_not_found"),
            _ => None,
        };
        let mut frames = match refusal {
            Some(code) => vec![json!({ "type": "error", "status": 400, "error": { "code": code, "message": code } })],
            None => response_frames(&id, &output_items(state.sockets, turn)),
        };
        if state.sockets == SocketBehavior::DropsMidResponse {
            frames.truncate(2);
        }

        for frame in std::iter::once(json!({ "type": "codex.rate_limits" })).chain(frames) {
            if connection.send(Message::text(frame.to_string())).await.is_err() {
                return;
            }
        }
        if matches!(state.sockets, SocketBehavior::ClosesWhenIdle | SocketBehavior::DropsMidResponse) {
            return;
        }
        cached = (refusal.is_none() && state.sockets != SocketBehavior::Forgets).then_some(id);
    }
}

fn output_items(sockets: SocketBehavior, turn: usize) -> Vec<Value> {
    let message = |text: &str| json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": text }] });
    match sockets {
        SocketBehavior::CallsTool if turn == 0 => {
            vec![json!({ "type": "function_call", "name": "read", "call_id": "call_1", "arguments": "{}" })]
        }
        SocketBehavior::SplitsReply => vec![message("Working on it. "), message("Done.")],
        SocketBehavior::UnknownOutput => vec![message("hi"), json!({ "type": "web_search_call", "id": "ws_1" })],
        _ => vec![message("hi")],
    }
}

fn response_frames(id: &str, output: &[Value]) -> Vec<Value> {
    let mut frames = vec![json!({ "type": "response.created", "response": { "id": id } })];
    for (output_index, item) in output.iter().enumerate() {
        if let Some(text) = item["content"][0]["text"].as_str() {
            frames.push(json!({ "type": "response.output_text.delta", "delta": text }));
        }
        frames.push(json!({ "type": "response.output_item.done", "output_index": output_index, "item": item }));
    }
    frames.push(json!({ "type": "response.completed", "response": { "id": id, "status": "completed" } }));
    frames
}
