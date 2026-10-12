use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, Method, header::AUTHORIZATION},
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use mcp_utils::client::{ClientOptions, McpClient, Transport};
use mcp_utils::model::{DiscoverResult, ErrorCode, ErrorData, ProtocolVersion, ServerCapabilities, ServerConfig};
use reqwest::header::{HeaderMap as ConfiguredHeaders, HeaderValue};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use std::future::{Future, ready};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};

#[tokio::test]
async fn every_request_of_a_session_sends_the_configured_headers() {
    let mut server = SessionServer::start().await;
    let transport = Transport::Http { url: server.url.clone(), headers: configured_headers(), oauth: None };
    let client = McpClient::connect("remote", transport, &ClientOptions::default()).await.unwrap();

    let mut requests = server.requests_until(Method::GET).await;
    client.close().await;
    requests.extend(server.received());

    for method in [Method::POST, Method::GET, Method::DELETE] {
        let (_, headers) =
            requests.iter().find(|(seen, _)| *seen == method).unwrap_or_else(|| panic!("the client sent no {method}"));
        assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1, "{method}");
        assert_eq!(headers[AUTHORIZATION], "Sentry-Bearer secret", "{method}");
        assert_eq!(headers["x-api-key"], "custom-secret", "{method}");
    }
}

struct SessionServer {
    url: String,
    requests: mpsc::UnboundedReceiver<(Method, HeaderMap)>,
    task: JoinHandle<()>,
}

impl SessionServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::unbounded_channel();
        let mcp = StreamableHttpService::new(
            || Ok(LegacyServer),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let app = Router::new().nest_service("/mcp", mcp).layer(from_fn_with_state(sender, record));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, requests, task }
    }

    async fn requests_until(&mut self, last: Method) -> Vec<(Method, HeaderMap)> {
        let mut requests = Vec::new();
        loop {
            let request = self.requests.recv().await.unwrap();
            let done = request.0 == last;
            requests.push(request);
            if done {
                return requests;
            }
        }
    }

    fn received(&mut self) -> Vec<(Method, HeaderMap)> {
        std::iter::from_fn(|| self.requests.try_recv().ok()).collect()
    }
}

impl Drop for SessionServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct LegacyServer;

impl ServerHandler for LegacyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, ErrorData>> + Send + '_ {
        ready(Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "server/discover is not supported", None)))
    }
}

async fn record(
    State(sender): State<mpsc::UnboundedSender<(Method, HeaderMap)>>,
    request: Request,
    next: Next,
) -> Response {
    sender.send((request.method().clone(), request.headers().clone())).unwrap();
    next.run(request).await
}

fn configured_headers() -> ConfiguredHeaders {
    ConfiguredHeaders::from_iter([
        (AUTHORIZATION, HeaderValue::from_static("Sentry-Bearer secret")),
        (HeaderName::from_static("x-api-key"), HeaderValue::from_static("custom-secret")),
    ])
}
