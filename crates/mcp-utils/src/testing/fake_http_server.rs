use crate::client::{HeaderMap, HttpAuth, McpClient, Transport};
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{OriginalUri, Request, State};
use axum::http::{Method, StatusCode, header::HOST};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Serves an MCP server over streamable HTTP at `/mcp` and records every request it receives there.
pub struct FakeHttpServer {
    url: String,
    requests: watch::Receiver<Vec<RecordedRequest>>,
    task: JoinHandle<()>,
}

/// A request as the server received it.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl FakeHttpServer {
    pub async fn start(server: impl ServerHandler + Clone) -> Self {
        Self::start_guarded(server, |_| Ok::<_, StatusCode>(())).await
    }

    /// Like [`Self::start`], but answers requests that `guard` rejects with its response.
    pub async fn start_guarded<R: IntoResponse>(
        server: impl ServerHandler + Clone,
        guard: impl Fn(&RecordedRequest) -> Result<(), R> + Send + Sync + 'static,
    ) -> Self {
        Self::serve(server, Router::new(), guard).await
    }

    pub fn transport(&self, headers: HeaderMap, auth: HttpAuth) -> Transport {
        Transport::Http { url: self.url.clone(), headers, auth }
    }

    /// Every request received so far, including rejected ones.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.borrow().clone()
    }

    /// Every request of a legacy session: waits for `client` to open its GET stream, then closes it so it sends DELETE.
    pub async fn record_session(&self, client: McpClient) -> Vec<RecordedRequest> {
        let mut requests = self.requests.clone();
        let opened = requests.wait_for(|requests| requests.iter().any(|request| request.method == Method::GET)).await;
        drop(opened.expect("the fake HTTP server stopped"));
        client.close().await;
        self.requests()
    }

    /// Like [`Self::start_guarded`], and also serves `routes` beside the guarded `/mcp` endpoint.
    pub(super) async fn serve<R: IntoResponse>(
        server: impl ServerHandler + Clone,
        routes: Router,
        guard: impl Fn(&RecordedRequest) -> Result<(), R> + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind the fake HTTP server");
        let url = format!("http://{}/mcp", listener.local_addr().expect("a bound address"));
        let (sender, requests) = watch::channel(Vec::new());
        let guard: Guard = Box::new(move |request| guard(request).err().map(IntoResponse::into_response));
        let mcp = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let app = Router::new()
            .nest_service("/mcp", mcp)
            .layer(from_fn_with_state(Arc::new(Recorder { guard, requests: sender }), record))
            .merge(routes);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve the fake HTTP server");
        });
        Self { url, requests, task }
    }
}

impl Drop for FakeHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The origin a request was sent to, e.g. `http://127.0.0.1:4000`.
pub(super) fn origin(headers: &HeaderMap) -> String {
    let host = headers.get(HOST).and_then(|host| host.to_str().ok()).unwrap_or_default();
    format!("http://{host}")
}

/// Returns the response that rejects a request, or `None` to accept it.
type Guard = Box<dyn Fn(&RecordedRequest) -> Option<Response> + Send + Sync>;

struct Recorder {
    guard: Guard,
    requests: watch::Sender<Vec<RecordedRequest>>,
}

async fn record(State(recorder): State<Arc<Recorder>>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("read the request body");
    let path = parts.extensions.get::<OriginalUri>().map_or(&parts.uri, |original| &original.0);
    let received = RecordedRequest {
        method: parts.method.clone(),
        url: format!("{}{path}", origin(&parts.headers)),
        headers: parts.headers.clone(),
        body: body.clone(),
    };
    let rejection = (recorder.guard)(&received);
    recorder.requests.send_modify(|requests| requests.push(received));
    match rejection {
        Some(rejection) => rejection,
        None => next.run(Request::from_parts(parts, Body::from(body))).await,
    }
}
