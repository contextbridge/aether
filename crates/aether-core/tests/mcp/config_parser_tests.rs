use mcp_utils::client::Transport;
use mcp_utils::config::{McpConfig, McpServerConfig, ParseError};
use reqwest::header::AUTHORIZATION;
use std::env;
use std::num::NonZeroU16;
use utils::variables::Vars;

fn parse_servers(json: &str) -> Result<Vec<(String, Transport)>, ParseError> {
    let vars = Vars::new();
    McpConfig::from_json(json)?
        .servers
        .into_iter()
        .map(|(name, config)| {
            let transport = match config {
                McpServerConfig::Stdio(config) => config.into_transport(&vars)?,
                McpServerConfig::Remote(config) => config.into_transport(&vars)?,
                McpServerConfig::InMemory(_) => panic!("in-memory servers have no transport"),
            };
            Ok((name, transport))
        })
        .collect()
}

fn parse_one(json: &str) -> (String, Transport) {
    let mut servers = parse_servers(json).unwrap();
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

fn assert_http(server: (String, Transport), expected_name: &str, expected_url: &str) -> Transport {
    let (name, transport) = server;
    match &transport {
        Transport::Http { url, .. } => {
            assert_eq!(name, expected_name);
            assert_eq!(url, expected_url);
        }
        other => panic!("Expected Http config, got {other:?}"),
    }
    transport
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
        let (name, transport) = parse_one(&json);
        match transport {
            Transport::Stdio { command, args, env } => {
                assert_eq!(name, "githubMcp");
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "@modelcontextprotocol/server-github"]);
                assert_eq!(env.get("GITHUB_TOKEN").unwrap(), "test_token");
            }
            other => panic!("Expected Stdio config, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn test_parse_http_oauth_config() {
    let json = server_json(
        "slack",
        r#"{
            "type": "http",
            "url": "https://mcp.slack.com/mcp",
            "oauth": {
                "clientId": "1601185624273.8899143856786",
                "callbackPort": 3118
            }
        }"#,
    );

    let (_, transport) = parse_one(&json);
    match transport {
        Transport::Http { oauth: Some(oauth), .. } => {
            assert_eq!(oauth.client_id.as_deref(), Some("1601185624273.8899143856786"));
            assert_eq!(oauth.callback_port.map(NonZeroU16::get), Some(3118));
        }
        other => panic!("Expected HTTP OAuth config, got {other:?}"),
    }
}

#[test]
fn test_rejects_zero_oauth_callback_port() {
    let json = server_json(
        "bad",
        r#"{
            "type": "http",
            "url": "https://example.com/mcp",
            "oauth": { "clientId": "client", "callbackPort": 0 }
        }"#,
    );

    assert!(McpConfig::from_json(&json).is_err());
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
    let transport = with_env!(
        [("API_TOKEN", "secret_token")],
        assert_http(parse_one(&json), "mcpMesh", "http://localhost:3000/mcp")
    );
    if let Transport::Http { headers, .. } = transport {
        assert_eq!(headers[AUTHORIZATION], "Bearer secret_token");
    }

    let json = server_json("sseServer", r#"{ "type": "sse", "url": "http://localhost:4000/sse", "headers": {} }"#);
    assert_http(parse_one(&json), "sseServer", "http://localhost:4000/sse");
}

#[tokio::test]
async fn test_missing_env_var_error() {
    let json = server_json("test", r#"{ "type": "stdio", "command": "$MISSING_VAR", "args": [] }"#);
    match parse_servers(&json).unwrap_err() {
        ParseError::VarError(_) => (),
        other => panic!("Expected VarError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_in_memory_config_is_declarative() {
    let json = server_json("test", r#"{ "type": "in-memory", "args": ["--root", "${WORKSPACE}"] }"#);
    let vars = Vars::new().with("WORKSPACE", "/workspace");
    let Some(McpServerConfig::InMemory(config)) = McpConfig::from_json(&json).unwrap().servers.remove("test") else {
        panic!("expected in-memory server");
    };
    let config = config.expand(&vars).unwrap();
    assert_eq!(config.args, ["--root", "/workspace"]);
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
        assert_eq!(parse_servers(json).unwrap().len(), 2);
    });
}

#[tokio::test]
async fn test_env_var_in_url() {
    let json = server_json("test", r#"{ "type": "http", "url": "http://${HOST}:${PORT}/mcp" }"#);
    with_env!([("HOST", "localhost"), ("PORT", "8080")], {
        assert_http(parse_one(&json), "test", "http://localhost:8080/mcp");
    });
}

#[tokio::test]
async fn test_parse_per_server_defer_tools_config() {
    let json = r#"{
        "servers": {
            "github": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "deferTools": true
            },
            "sentry": { "type": "http", "url": "https://sentry.example.com/mcp" }
        }
    }"#;
    let servers = McpConfig::from_json(json).unwrap().servers;
    assert_eq!(servers.len(), 2);
    assert!(servers["github"].defer_tools().has_deferred_tools());
    assert!(!servers["sentry"].defer_tools().has_deferred_tools());
}

#[test]
fn test_rejects_unknown_server_type() {
    let json = server_json("outer", r#"{ "type": "unknown", "servers": { "bad": { "type": "in-memory" } } }"#);
    assert!(McpConfig::from_json(&json).is_err());
}
