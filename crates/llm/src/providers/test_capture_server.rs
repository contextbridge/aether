use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

pub(crate) struct CaptureServer {
    pub(crate) base_url: String,
    receiver: mpsc::UnboundedReceiver<CapturedRequest>,
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
}

pub(crate) struct CapturedRequest {
    pub(crate) path: String,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Value,
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
        let app = Router::new()
            .route("/responses", post(capture))
            .route("/chat/completions", post(capture))
            .route("/v1/chat/completions", post(capture))
            .with_state(Arc::new(CaptureState { sender, response: spec }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { base_url, receiver }
    }

    pub(crate) async fn captured(&mut self) -> CapturedRequest {
        self.receiver.recv().await.expect("no request captured")
    }
}

const RESPONSES_FIXTURE: &str = include_str!("../../tests/fixtures/openai_responses/01_minimal.sse");
const CHAT_COMPLETIONS_FIXTURE: &str = include_str!("../../tests/fixtures/openai/01_minimal.sse");
const OPENROUTER_FIXTURE: &str = include_str!("../../tests/fixtures/openrouter/01_minimal.sse");

async fn capture(
    State(state): State<Arc<CaptureState>>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
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
