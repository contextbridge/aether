use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A local HTTP server that captures OpenAI-compatible POST requests and replies
/// with a protocol-specific SSE fixture, letting provider tests assert the exact wire request.
pub(crate) struct CaptureServer {
    pub(crate) base_url: String,
    receiver: mpsc::UnboundedReceiver<CapturedRequest>,
}

struct CaptureState {
    sender: mpsc::UnboundedSender<CapturedRequest>,
    response: &'static str,
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
        let (sender, receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/responses", post(capture))
            .route("/chat/completions", post(capture))
            .route("/v1/chat/completions", post(capture))
            .with_state(Arc::new(CaptureState { sender, response }));
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
    ([(CONTENT_TYPE, "text/event-stream")], state.response)
}
