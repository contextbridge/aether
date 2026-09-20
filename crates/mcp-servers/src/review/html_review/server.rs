use crate::review::tools::ReviewArtifactOutput;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{HeaderName, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use reqwest::Url;
use std::collections::HashMap;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

const OVERLAY_CSS: &str = include_str!("assets/overlay.css");
const OVERLAY_JS: &str = include_str!("assets/overlay.js");

pub enum Artifact {
    Document { html: String, assets: Option<PathBuf> },
    App(Url),
}

#[derive(Clone, Default)]
pub struct PendingReviews(Arc<Mutex<HashMap<String, ReviewServer>>>);

impl PendingReviews {
    pub fn insert(&self, server: ReviewServer) {
        self.0.lock().unwrap().insert(server.token.clone(), server);
    }

    pub fn take(&self, token: &str) -> Option<ReviewServer> {
        self.0.lock().unwrap().remove(token)
    }
}

pub struct ReviewServer {
    url: String,
    token: String,
    result_rx: oneshot::Receiver<ReviewArtifactOutput>,
    handle: JoinHandle<()>,
}

impl ReviewServer {
    pub fn start(artifact: Artifact) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = Uuid::new_v4().simple().to_string();

        let (url, router): (String, Router<Arc<ServerState>>) = match artifact {
            Artifact::Document { html, assets } => {
                let page = render_document(&html, &token);
                let router = Router::new().route("/", get(move || std::future::ready(Html(page.clone()))));
                let router = match assets {
                    Some(directory) => router.fallback_service(ServeDir::new(directory)),
                    None => router,
                };
                (format!("http://127.0.0.1:{port}/"), router)
            }
            Artifact::App(target) => {
                let query = target.query().map_or_else(String::new, |query| format!("?{query}"));
                let proxy = Proxy::new(target.origin().ascii_serialization(), token.clone())?;
                let router = Router::new().fallback(move |request: Request| proxy.clone().forward(request));
                (format!("http://127.0.0.1:{port}{}{query}", target.path()), router)
            }
        };

        let (result_tx, result_rx) = oneshot::channel();
        let shutdown = CancellationToken::new();
        let state = Arc::new(ServerState {
            token: token.clone(),
            result: Mutex::new(Some(result_tx)),
            shutdown: shutdown.clone(),
        });
        let router =
            router.route("/submit", post(serve_submit)).layer(DefaultBodyLimit::max(MAX_BODY_BYTES)).with_state(state);

        let listener = tokio::net::TcpListener::from_std(listener)?;
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await;
        });

        Ok(Self { url, token, result_rx, handle })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn wait(mut self, cancelled: CancellationToken) -> ReviewArtifactOutput {
        let submitted = tokio::select! {
            result = &mut self.result_rx => result.ok(),
            () = cancelled.cancelled() => None,
        };
        match submitted {
            Some(output) => {
                let _ = (&mut self.handle).await;
                output
            }
            None => ReviewArtifactOutput::Cancelled,
        }
    }
}

impl Drop for ReviewServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct ServerState {
    token: String,
    result: Mutex<Option<oneshot::Sender<ReviewArtifactOutput>>>,
    shutdown: CancellationToken,
}

#[derive(Clone)]
struct Proxy {
    client: reqwest::Client,
    origin: String,
    token: String,
}

impl Proxy {
    fn new(origin: String, token: String) -> io::Result<Self> {
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(io::Error::other)?;
        Ok(Self { client, origin, token })
    }

    async fn forward(self, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let path_and_query = parts.uri.path_and_query().map_or("/", |path| path.as_str());
        let Ok(body) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        };

        let mut upstream = self.client.request(parts.method, format!("{}{path_and_query}", self.origin)).body(body);
        for (name, value) in &parts.headers {
            if !is_hop_by_hop(name) && *name != header::HOST && *name != header::ACCEPT_ENCODING {
                upstream = upstream.header(name, value);
            }
        }
        let response = match upstream.header(header::ACCEPT_ENCODING, "identity").send().await {
            Ok(response) => response,
            Err(error) => {
                return (StatusCode::BAD_GATEWAY, format!("Could not reach {}: {error}", self.origin)).into_response();
            }
        };

        let status = response.status();
        let headers = response.headers().clone();
        let is_html = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|content_type| content_type.starts_with("text/html"));
        let Ok(bytes) = response.bytes().await else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        let body = if is_html {
            Body::from(render_document(&String::from_utf8_lossy(&bytes), &self.token))
        } else {
            Body::from(bytes)
        };

        let stripped = [header::CONTENT_LENGTH, header::CONTENT_ENCODING, header::CONTENT_SECURITY_POLICY];
        let mut builder = Response::builder().status(status);
        for (name, value) in &headers {
            if !is_hop_by_hop(name) && !stripped.contains(name) {
                builder = builder.header(name, value);
            }
        }
        builder.body(body).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
    }
}

async fn serve_submit(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<HashMap<String, String>>,
    Json(output): Json<ReviewArtifactOutput>,
) -> StatusCode {
    if query.get("token") != Some(&state.token) {
        return StatusCode::FORBIDDEN;
    }
    let Some(sender) = state.result.lock().unwrap().take() else {
        return StatusCode::GONE;
    };
    if sender.send(output).is_err() {
        return StatusCode::GONE;
    }
    state.shutdown.cancel();
    StatusCode::NO_CONTENT
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(name.as_str(), "connection" | "keep-alive" | "transfer-encoding" | "upgrade" | "te" | "trailer")
}

fn render_document(html: &str, token: &str) -> String {
    let injection = format!(
        "<style id=\"aether-review-style\">{OVERLAY_CSS}</style><script data-token=\"{token}\">{OVERLAY_JS}</script>"
    );
    match html.to_ascii_lowercase().rfind("</body") {
        Some(index) => format!("{}{}{}", &html[..index], injection, &html[index..]),
        None => format!("{html}{injection}"),
    }
}
