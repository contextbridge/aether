use agent_client_protocol::schema::v2::{HttpHeader, McpServer};
use mcp_utils::client::{HeaderMap, Transport};
use mcp_utils::gateway::{ServerSpec, ToolExposure};

pub fn map_acp_mcp_servers(servers: Vec<McpServer>) -> Vec<ServerSpec> {
    servers
        .into_iter()
        .filter_map(|s| {
            try_map_mcp_server(s).or_else(|| {
                tracing::warn!("Unsupported ACP MCP transport or invalid HTTP headers, skipping server");
                None
            })
        })
        .collect()
}

fn try_map_mcp_server(server: McpServer) -> Option<ServerSpec> {
    use McpServer::{Http, Stdio};
    let (name, transport) = match server {
        Stdio(stdio) => (
            stdio.name,
            Transport::Stdio {
                command: stdio.command.0.to_string_lossy().into_owned(),
                args: stdio.args,
                env: stdio.env.into_iter().map(|e| (e.name, e.value)).collect(),
            },
        ),
        Http(http) => (http.name, Transport::Http { url: http.url, headers: header_map(&http.headers)?, oauth: None }),
        _ => return None,
    };
    Some(ServerSpec { name, transport, exposure: ToolExposure::ModelVisible })
}

fn header_map(headers: &[HttpHeader]) -> Option<HeaderMap> {
    headers.iter().map(|header| Some((header.name.parse().ok()?, header.value.parse().ok()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2 as acp;

    #[test]
    fn test_map_acp_stdio_server() {
        let server = acp::McpServer::Stdio(
            acp::McpServerStdio::new("my-server", acp::AbsolutePath::new("/usr/bin/server"))
                .args(vec!["--port".into(), "8080".into()])
                .env(vec![acp::EnvVariable::new("FOO", "bar")]),
        );

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            Transport::Stdio { command, args, env } => {
                assert_eq!(configs[0].name, "my-server");
                assert_eq!(command, "/usr/bin/server");
                assert_eq!(args, &["--port", "8080"]);
                assert_eq!(env.get("FOO").unwrap(), "bar");
            }
            other => panic!("Expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn test_map_acp_http_server() {
        let server = acp::McpServer::Http(
            acp::McpServerHttp::new("http-server", "https://example.com/mcp")
                .headers(vec![acp::HttpHeader::new("Authorization", "Bearer token123")]),
        );

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            Transport::Http { url, headers, .. } => {
                assert_eq!(configs[0].name, "http-server");
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(headers["authorization"], "Bearer token123");
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    fn http_headers_preserve_authorization_schemes_and_custom_values() {
        for input in ["Bearer token123", "bearer token123", "Basic abc", "Token foo"] {
            let server =
                acp::McpServer::Http(acp::McpServerHttp::new("http-server", "https://example.com/mcp").headers(vec![
                    acp::HttpHeader::new("Authorization", input),
                    acp::HttpHeader::new("X-API-Key", "secret"),
                ]));
            let configs = map_acp_mcp_servers(vec![server]);
            match &configs[0].transport {
                Transport::Http { headers, .. } => {
                    assert_eq!(headers["authorization"], input);
                    assert_eq!(headers["x-api-key"], "secret");
                }
                other => panic!("Expected Http, got {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_headers_skip_the_server_instead_of_dropping_credentials() {
        let server = acp::McpServer::Http(
            acp::McpServerHttp::new("invalid", "https://example.com/mcp")
                .headers(vec![acp::HttpHeader::new("Authorization", "invalid\nvalue")]),
        );
        assert!(map_acp_mcp_servers(vec![server]).is_empty());
    }

    #[test]
    fn omitted_stdio_options_and_http_headers_default_to_empty() {
        let servers = serde_json::from_value(serde_json::json!([
            {"type": "stdio", "name": "local", "command": "/usr/bin/server"},
            {"type": "http", "name": "remote", "url": "https://example.com/mcp"}
        ]))
        .unwrap();
        let configs = map_acp_mcp_servers(servers);
        let Transport::Stdio { args, env, .. } = &configs[0].transport else { panic!("expected stdio") };
        assert!(args.is_empty() && env.is_empty());
        let Transport::Http { headers, .. } = &configs[1].transport else { panic!("expected http") };
        assert!(headers.is_empty());
    }

    #[test]
    fn unsupported_transport_is_skipped() {
        let server = serde_json::from_value(serde_json::json!({
            "type": "sse", "name": "unsupported", "url": "https://example.com/sse"
        }))
        .unwrap();
        assert!(map_acp_mcp_servers(vec![server]).is_empty());
    }
}
