use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderName, Method, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::any,
};
use mcp_utils::client::{McpConfig, McpHttpConfig, McpTransport};
use rmcp::transport::streamable_http_client::{StreamableHttpClient, StreamableHttpClientTransportConfig};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use utils::variables::Vars;

#[test]
fn configured_authorization_schemes_and_header_casing_are_preserved() {
    for scheme in ["Sentry-Bearer", "Bearer", "bearer", "Basic", "Token"] {
        for header_name in ["Authorization", "authorization", "AUTHORIZATION"] {
            let config = parse_config(
                "http://localhost/mcp",
                &serde_json::json!({
                    header_name: format!("{scheme} $TOKEN"),
                    "X-API-Key": "$API_KEY"
                }),
            );
            assert!(config.transport.auth_header.is_none());
            assert_eq!(config.transport.custom_headers[&AUTHORIZATION], format!("{scheme} secret"));
            assert_eq!(config.transport.custom_headers[&HeaderName::from_static("x-api-key")], "custom-secret");
        }
    }
}

#[tokio::test]
async fn configured_headers_are_sent_verbatim_for_all_http_methods() {
    let mut server = HeaderCaptureServer::start().await;
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let transport = parse_config(
            &server.url,
            &serde_json::json!({
                "Authorization": "Sentry-Bearer $TOKEN",
                "X-API-Key": "$API_KEY"
            }),
        )
        .transport;

        send_request(method.clone(), transport).await;

        let (received_method, headers) = server.next_request().await;
        assert_eq!(received_method, method);
        assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1, "{method}");
        assert_eq!(headers[AUTHORIZATION], "Sentry-Bearer secret", "{method}");
        assert_eq!(headers["x-api-key"], "custom-secret", "{method}");
    }
}

struct HeaderCaptureServer {
    url: String,
    requests: mpsc::UnboundedReceiver<(Method, HeaderMap)>,
    task: JoinHandle<()>,
}

impl HeaderCaptureServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::unbounded_channel();
        let app = Router::new().route("/mcp", any(capture_request)).with_state(sender);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, requests, task }
    }

    async fn next_request(&mut self) -> (Method, HeaderMap) {
        self.requests.recv().await.unwrap()
    }
}

impl Drop for HeaderCaptureServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn capture_request(
    State(sender): State<mpsc::UnboundedSender<(Method, HeaderMap)>>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    sender.send((method.clone(), headers)).unwrap();
    match method {
        Method::POST => StatusCode::ACCEPTED.into_response(),
        Method::GET => ([("content-type", "text/event-stream")], "").into_response(),
        Method::DELETE => StatusCode::OK.into_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

fn parse_config(url: &str, headers: &serde_json::Value) -> McpHttpConfig {
    let json = serde_json::json!({"servers": {"remote": {"type": "http", "url": url, "headers": headers}}});
    let vars = Vars::new().with("TOKEN", "secret").with("API_KEY", "custom-secret");
    let mut servers = McpConfig::from_json(&json.to_string()).unwrap().into_servers(&vars).unwrap();
    let McpTransport::Http(config) = servers.remove(0).transport else { panic!("expected HTTP transport") };
    config
}

async fn send_request(method: Method, transport: StreamableHttpClientTransportConfig) {
    let client = reqwest::Client::new();
    match method {
        Method::POST => {
            let message =
                serde_json::from_value(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                    .unwrap();
            client
                .post_message(transport.uri, message, None, transport.auth_header, transport.custom_headers)
                .await
                .unwrap();
        }
        Method::GET => {
            let _stream = client
                .get_stream(
                    transport.uri,
                    Some("session".into()),
                    None,
                    transport.auth_header,
                    transport.custom_headers,
                )
                .await
                .unwrap();
        }
        Method::DELETE => {
            client
                .delete_session(transport.uri, "session".into(), transport.auth_header, transport.custom_headers)
                .await
                .unwrap();
        }
        _ => panic!("unsupported test method: {method}"),
    }
}
