use mcp_utils::client::{McpConfig, McpServer, McpTransport, ParseError};
use std::collections::HashMap;
use std::env;

async fn parse_servers(json: &str) -> Result<Vec<McpServer>, ParseError> {
    McpConfig::from_json(json).unwrap().into_servers(&HashMap::new()).await
}

async fn parse_one(json: &str) -> McpServer {
    let mut servers = parse_servers(json).await.unwrap();
    assert_eq!(servers.len(), 1);
    servers.remove(0)
}

fn server_json(name: &str, body: &str) -> String {
    format!(r#"{{ "servers": {{ "{name}": {body} }} }}"#)
}

macro_rules! with_env {
    ([$( ($k:expr, $v:expr) ),+ $(,)?], $body:expr) => {{
        unsafe { $( env::set_var($k, $v); )+ }
        let _result = $body;
        unsafe { $( env::remove_var($k); )+ }
        _result
    }};
}

fn assert_http(server: McpServer, expected_name: &str, expected_url: &str) -> McpServer {
    match &server.transport {
        McpTransport::Http { config: c } => {
            assert_eq!(server.name, expected_name);
            assert_eq!(c.uri.to_string(), expected_url);
        }
        other => panic!("Expected Http config, got {other:?}"),
    }
    server
}

#[tokio::test]
async fn test_parse_stdio_config() {
    let json = server_json(
        "githubMcp",
        r#"{
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"],
            "env": { "GITHUB_TOKEN": "$GITHUB_TOKEN" }
        }"#,
    );
    with_env!([("GITHUB_TOKEN", "test_token")], {
        let server = parse_one(&json).await;
        assert!(!server.proxy);
        match server.transport {
            McpTransport::Stdio { command, args, env } => {
                assert_eq!(server.name, "githubMcp");
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "@modelcontextprotocol/server-github"]);
                assert_eq!(env.get("GITHUB_TOKEN").unwrap(), "test_token");
            }
            other => panic!("Expected Stdio config, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn test_parse_http_and_sse_configs() {
    let json = server_json(
        "mcpMesh",
        r#"{
            "type": "http",
            "url": "http://localhost:3000/mcp",
            "headers": { "Authorization": "Bearer $API_TOKEN" }
        }"#,
    );
    let cfg = with_env!(
        [("API_TOKEN", "secret_token")],
        assert_http(parse_one(&json).await, "mcpMesh", "http://localhost:3000/mcp")
    );
    if let McpTransport::Http { config: c } = cfg.transport {
        assert_eq!(c.auth_header.as_ref().unwrap(), "Bearer secret_token");
    }

    let json = server_json("sseServer", r#"{ "type": "sse", "url": "http://localhost:4000/sse", "headers": {} }"#);
    assert_http(parse_one(&json).await, "sseServer", "http://localhost:4000/sse");
}

#[tokio::test]
async fn test_missing_env_var_error() {
    let json = server_json("test", r#"{ "type": "stdio", "command": "$MISSING_VAR", "args": [] }"#);
    match parse_servers(&json).await.unwrap_err() {
        ParseError::VarError(_) => (),
        other => panic!("Expected VarError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_factory_not_found_error() {
    let json = server_json("test", r#"{ "type": "in-memory" }"#);
    match parse_servers(&json).await.unwrap_err() {
        ParseError::FactoryNotFound(name) => assert_eq!(name, "test"),
        other => panic!("Expected FactoryNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn test_multiple_servers() {
    let json = r#"{
        "servers": {
            "server1": { "type": "stdio", "command": "node", "args": ["server.js"] },
            "server2": {
                "type": "http",
                "url": "http://localhost:3000/mcp",
                "headers": { "Authorization": "$TOKEN" }
            }
        }
    }"#;
    with_env!([("TOKEN", "test")], {
        assert_eq!(parse_servers(json).await.unwrap().len(), 2);
    });
}

#[tokio::test]
async fn test_env_var_in_url() {
    let json = server_json("test", r#"{ "type": "http", "url": "http://${HOST}:${PORT}/mcp" }"#);
    with_env!([("HOST", "localhost"), ("PORT", "8080")], {
        assert_http(parse_one(&json).await, "test", "http://localhost:8080/mcp");
    });
}

#[tokio::test]
async fn test_parse_per_server_proxy_config() {
    let json = r#"{
        "servers": {
            "github": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "proxy": true
            },
            "sentry": { "type": "http", "url": "https://sentry.example.com/mcp" }
        }
    }"#;
    let servers = parse_servers(json).await.unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.iter().find(|s| s.name == "github").unwrap().proxy);
    assert!(!servers.iter().find(|s| s.name == "sentry").unwrap().proxy);
}

#[test]
fn test_rejects_proxy_server_type() {
    let json = server_json("outer", r#"{ "type": "proxy", "servers": { "bad": { "type": "in-memory" } } }"#);
    assert!(McpConfig::from_json(&json).is_err());
}
