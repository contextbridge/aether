use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderName, Method, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::any,
};
use mcp_utils::client::{ClientOptions, McpClient, Transport};
use mcp_utils::config::RemoteServerConfig;
use reqwest::header::HeaderMap as ConfiguredHeaders;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use utils::variables::Vars;

#[test]
fn configured_authorization_schemes_and_header_casing_are_preserved() {
    for scheme in ["Sentry-Bearer", "Bearer", "bearer", "Basic", "Token"] {
        for header_name in ["Authorization", "authorization", "AUTHORIZATION"] {
            let (_, headers) = parse_config(
                "http://localhost/mcp",
                &serde_json::json!({
                    header_name: format!("{scheme} $TOKEN"),
                    "X-API-Key": "$API_KEY"
                }),
            );
            assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1);
            assert_eq!(headers[AUTHORIZATION], format!("{scheme} secret"));
            assert_eq!(headers[HeaderName::from_static("x-api-key")], "custom-secret");
        }
    }
}

#[tokio::test]
async fn configured_headers_are_sent_verbatim_for_all_http_methods() {
    let mut server = HeaderCaptureServer::start().await;
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let (url, headers) = parse_config(
            &server.url,
            &serde_json::json!({
                "Authorization": "Sentry-Bearer $TOKEN",
                "X-API-Key": "$API_KEY"
            }),
        );

        send_request(method.clone(), url, headers).await;

        let (received_method, headers) = server.next_request().await;
        assert_eq!(received_method, method);
        assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1, "{method}");
        assert_eq!(headers[AUTHORIZATION], "Sentry-Bearer secret", "{method}");
        assert_eq!(headers["x-api-key"], "custom-secret", "{method}");
    }
}

#[tokio::test]
async fn client_handshake_sends_configured_headers_verbatim() {
    let mut server = HeaderCaptureServer::start().await;
    let (url, headers) = parse_config(
        &server.url,
        &serde_json::json!({
            "Authorization": "Sentry-Bearer $TOKEN",
            "X-API-Key": "$API_KEY"
        }),
    );
    let connect = tokio::spawn(async move {
        McpClient::connect("remote", Transport::Http { url, headers, oauth: None }, &ClientOptions::default()).await
    });

    let (method, headers) = server.next_request().await;
    connect.abort();

    assert_eq!(method, Method::POST);
    assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1);
    assert_eq!(headers[AUTHORIZATION], "Sentry-Bearer secret");
    assert_eq!(headers["x-api-key"], "custom-secret");
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

fn parse_config(url: &str, headers: &serde_json::Value) -> (String, ConfiguredHeaders) {
    let config: RemoteServerConfig =
        serde_json::from_value(serde_json::json!({"type": "http", "url": url, "headers": headers})).unwrap();
    let vars = Vars::new().with("TOKEN", "secret").with("API_KEY", "custom-secret");
    let Transport::Http { url, headers, .. } = config.into_transport(&vars).unwrap() else {
        panic!("expected HTTP transport")
    };
    (url, headers)
}

async fn send_request(method: Method, url: String, headers: ConfiguredHeaders) {
    let client = reqwest::Client::new();
    let uri: std::sync::Arc<str> = url.into();
    let custom_headers = headers.into_iter().filter_map(|(name, value)| Some((name?, value))).collect();
    match method {
        Method::POST => {
            let message =
                serde_json::from_value(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                    .unwrap();
            client.post_message(uri, message, None, None, custom_headers).await.unwrap();
        }
        Method::GET => {
            let _stream = client.get_stream(uri, Some("session".into()), None, None, custom_headers).await.unwrap();
        }
        Method::DELETE => {
            client.delete_session(uri, "session".into(), None, custom_headers).await.unwrap();
        }
        _ => panic!("unsupported test method: {method}"),
    }
}
