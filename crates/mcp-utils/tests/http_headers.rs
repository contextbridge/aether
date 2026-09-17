use axum::{Json, Router, extract::Request, http::StatusCode, routing::post};
use mcp_utils::client::{McpConfig, McpTransport};
use rmcp::{
    ClientLifecycleMode, model::ProtocolVersion, serve_client_with_lifecycle, transport::StreamableHttpClientTransport,
};
use serde_json::{Value, json};
use utils::variables::Vars;

#[tokio::test]
async fn configured_proxy_headers_reach_http_server_on_each_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/mcp", post(require_proxy_headers));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = McpConfig::from_json(
        &json!({"servers": {"remote": {
            "type": "http", "url": format!("http://{address}/mcp"),
            "headers": {"X-aws-proxy-auth": "$TOKEN", "X-aws-proxy-port": "8080"}
        }}})
        .to_string(),
    )
    .unwrap();
    let servers = config.into_servers(&Vars::new().with("TOKEN", "private-token")).unwrap();
    let McpTransport::Http(config) = &servers[0].transport else { panic!("expected HTTP") };
    for _ in 0..2 {
        let transport = StreamableHttpClientTransport::from_config(config.transport.clone());
        let result = serve_client_with_lifecycle(
            (),
            transport,
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await;
        match result {
            Ok(client) => {
                assert!(client.list_all_tools().await.unwrap().is_empty());
                client.cancel().await.unwrap();
            }
            Err(error) => {
                server.abort();
                panic!("HTTP client failed: {error}");
            }
        }
    }
    server.abort();
}

#[test]
fn configured_headers_validate_and_expand_without_leaking_values() {
    let config = configured_headers(&json!({"authorization": "bEaReR $TOKEN", "X-Custom": "$TOKEN"}));
    let servers = config.into_servers(&Vars::new().with("TOKEN", "secret-token")).unwrap();
    let McpTransport::Http(config) = &servers[0].transport else { panic!("expected HTTP") };
    assert_eq!(config.transport.auth_header.as_deref(), Some("secret-token"));
    assert_eq!(config.transport.custom_headers[&reqwest::header::HeaderName::from_static("x-custom")], "secret-token");
    assert!(config.transport.custom_headers[&reqwest::header::HeaderName::from_static("x-custom")].is_sensitive());
    assert!(config.resolved_oauth().is_none());

    for headers in [
        json!({"bad name": "secret-token"}),
        json!({"X-Token": "secret-token\r\ninjected: true"}),
        json!({"Authorization": "secret-token\n"}),
        json!({"X-Token": "secret-token", "x-token": "other-secret"}),
        json!({"Mcp-Session-Id": "secret-token"}),
        json!({"Mcp-Protocol-Version": "secret-token"}),
        json!({"Content-Length": "secret-token"}),
    ] {
        let error = configured_headers(&headers).into_servers(&Vars::new()).unwrap_err();
        assert!(!error.to_string().contains("secret-token"));
        assert!(!format!("{error:?}").contains("secret-token"));
    }
}

fn configured_headers(headers: &Value) -> McpConfig {
    McpConfig::from_json(
        &json!({"servers": {"remote": {
            "type": "http", "url": "http://localhost/mcp", "headers": headers
        }}})
        .to_string(),
    )
    .unwrap()
}

async fn require_proxy_headers(request: Request) -> Result<Json<Value>, StatusCode> {
    let headers = request.headers();
    if headers.get("x-aws-proxy-auth").and_then(|v| v.to_str().ok()) != Some("private-token")
        || headers.get("x-aws-proxy-port").and_then(|v| v.to_str().ok()) != Some("8080")
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let bytes = axum::body::to_bytes(request.into_body(), 4096).await.unwrap();
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    let result = match request["method"].as_str() {
        Some("tools/list") => json!({"tools": []}),
        _ => {
            json!({"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "private"})
        }
    };
    Ok(Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result})))
}
