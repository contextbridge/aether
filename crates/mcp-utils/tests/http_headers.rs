use axum::http::{HeaderName, HeaderValue, Method, header::AUTHORIZATION};
use mcp_utils::client::{ClientOptions, HeaderMap, HttpAuth, McpClient};
use mcp_utils::testing::{FakeHttpServer, FakeMcpServer};

#[tokio::test]
async fn every_request_of_a_session_sends_the_configured_headers() {
    let server = FakeHttpServer::start(FakeMcpServer::new().with_legacy_protocol()).await;
    let transport = server.transport(configured_headers(), HttpAuth::None);
    let client = McpClient::connect("remote", transport, &ClientOptions::default()).await.unwrap();

    let requests = server.record_session(client).await;

    for method in [Method::POST, Method::GET, Method::DELETE] {
        let request = requests
            .iter()
            .find(|request| request.method == method)
            .unwrap_or_else(|| panic!("the client sent no {method}"));
        assert_eq!(request.headers.get_all(AUTHORIZATION).iter().count(), 1, "{method}");
        assert_eq!(request.headers[AUTHORIZATION], "Sentry-Bearer secret", "{method}");
        assert_eq!(request.headers["x-api-key"], "custom-secret", "{method}");
    }
}

fn configured_headers() -> HeaderMap {
    HeaderMap::from_iter([
        (AUTHORIZATION, HeaderValue::from_static("Sentry-Bearer secret")),
        (HeaderName::from_static("x-api-key"), HeaderValue::from_static("custom-secret")),
    ])
}
