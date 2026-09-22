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
use url::Position;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const SHELL_PATH: &str = "/__aether__/";
const SUBMIT_PATH: &str = "/__aether__/submit";

const PICKER_CSS: &str = include_str!("assets/picker.css");
const PANEL_CSS: &str = include_str!("assets/panel.css");
const PANEL_HTML: &str = include_str!("assets/panel.html");
const PICKER_JS: &str = include_str!("assets/picker.js");
const SHELL_JS: &str = include_str!("assets/shell.js");

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

        let (router, app_src, origin): (Router<Arc<ServerState>>, String, Option<String>) = match artifact {
            Artifact::Document { html, assets } => {
                let page = inject_picker(&html);
                let router = Router::new().route("/", get(move || std::future::ready(Html(page.clone()))));
                let router = match assets {
                    Some(directory) => router.fallback_service(
                        ServeDir::new(directory)
                            .not_found_service(Router::new().fallback(|| async { StatusCode::NOT_FOUND })),
                    ),
                    None => router,
                };
                (router, "/".to_string(), None)
            }
            Artifact::App(target) => {
                let app_src = target[Position::BeforePath..].to_string();
                let origin = target.origin().ascii_serialization();
                let proxy = Proxy::new(origin.clone())?;
                let router = Router::new().fallback(move |request: Request| proxy.clone().forward(request));
                (router, app_src, Some(origin))
            }
        };

        let (result_tx, result_rx) = oneshot::channel();
        let shutdown = CancellationToken::new();
        let state = Arc::new(ServerState {
            token: token.clone(),
            result: Mutex::new(Some(result_tx)),
            shutdown: shutdown.clone(),
        });
        let shell = render_shell(&token, &app_src, origin.as_deref());
        let router = router
            .route(SHELL_PATH, get(move || std::future::ready(Html(shell.clone()))))
            .route(SUBMIT_PATH, post(serve_submit))
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .with_state(state);

        let listener = tokio::net::TcpListener::from_std(listener)?;
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await;
        });

        Ok(Self { url: format!("http://127.0.0.1:{port}{SHELL_PATH}"), token, result_rx, handle })
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
}

impl Proxy {
    fn new(origin: String) -> io::Result<Self> {
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(io::Error::other)?;
        Ok(Self { client, origin })
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
        let body =
            if is_html { Body::from(inject_picker(&String::from_utf8_lossy(&bytes))) } else { Body::from(bytes) };

        let stripped = [
            header::CONTENT_LENGTH,
            header::CONTENT_ENCODING,
            header::CONTENT_SECURITY_POLICY,
            // The app is framed by the shell now, so its own framing policy would blank the review.
            header::X_FRAME_OPTIONS,
        ];
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

fn render_shell(token: &str, app_src: &str, origin: Option<&str>) -> String {
    let app_src = escape_attribute(app_src);
    let origin = origin.map_or_else(String::new, |origin| format!(" data-origin=\"{}\"", escape_attribute(origin)));
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Review</title><style>{PICKER_CSS}{PANEL_CSS}</style></head>\
         <body><iframe class=\"aether-app\" src=\"{app_src}\" title=\"Reviewed app\"></iframe>\
         {PANEL_HTML}\
         <script data-token=\"{token}\"{origin}>{SHELL_JS}</script></body></html>"
    )
}

fn inject_picker(html: &str) -> String {
    let injection = format!("<style>{PICKER_CSS}</style><script>{PICKER_JS}</script>");
    match html.to_ascii_lowercase().rfind("</body") {
        Some(index) => format!("{}{}{}", &html[..index], injection, &html[index..]),
        None => format!("{html}{injection}"),
    }
}

fn escape_attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}
