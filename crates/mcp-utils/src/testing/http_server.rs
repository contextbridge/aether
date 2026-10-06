use axum::{
    Router,
    extract::{Request, State},
    http::HeaderMap,
    middleware::{self, Next},
    response::Response,
};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Serves an rmcp handler over stateless streamable HTTP on a loopback port and
/// records the headers of every request it receives.
pub struct HttpTestServer {
    address: SocketAddr,
    headers: Arc<Mutex<Vec<HeaderMap>>>,
    task: JoinHandle<()>,
}

impl HttpTestServer {
    pub async fn start<S>(server: S) -> Self
    where
        S: ServerHandler + Clone + Send + Sync + 'static,
    {
        Self::start_at(SocketAddr::from(([127, 0, 0, 1], 0)), server).await
    }

    pub async fn start_at<S>(address: SocketAddr, server: S) -> Self
    where
        S: ServerHandler + Clone + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(address).await.expect("bind HTTP test server");
        let address = listener.local_addr().expect("HTTP test server address");
        let headers = Arc::new(Mutex::new(Vec::new()));
        let service = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(NeverSessionManager::default()),
            StreamableHttpServerConfig::default().with_legacy_session_mode(false),
        );
        let app = Router::new()
            .route_service("/mcp", service)
            .layer(middleware::from_fn_with_state(Arc::clone(&headers), record_headers));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve HTTP test server");
        });
        Self { address, headers, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn received_headers(&self) -> Vec<HeaderMap> {
        self.headers.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for HttpTestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn record_headers(State(headers): State<Arc<Mutex<Vec<HeaderMap>>>>, request: Request, next: Next) -> Response {
    headers.lock().unwrap_or_else(PoisonError::into_inner).push(request.headers().clone());
    next.run(request).await
}
