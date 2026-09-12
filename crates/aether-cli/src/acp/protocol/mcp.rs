use agent_client_protocol::schema::v2::{HttpHeader, McpServer};
use mcp_utils::client::{McpServer as RuntimeMcpServer, McpTransport, ToolExposure};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

/// Maps ACP MCP server definitions to internal MCP servers, skipping unsupported transports or invalid headers.
pub fn map_acp_mcp_servers(servers: Vec<McpServer>) -> Vec<RuntimeMcpServer> {
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

fn try_map_mcp_server(server: McpServer) -> Option<RuntimeMcpServer> {
    use McpServer::{Http, Stdio};
    match server {
        Stdio(stdio) => Some(RuntimeMcpServer::new(
            stdio.name,
            McpTransport::Stdio {
                command: stdio.command.0.to_string_lossy().into_owned(),
                args: stdio.args,
                env: stdio.env.into_iter().map(|e| (e.name, e.value)).collect(),
            },
            ToolExposure::ModelVisible,
        )),

        Http(http) => Some(RuntimeMcpServer::new(
            http.name,
            McpTransport::Http(http_config(http.url, &http.headers)?.into()),
            ToolExposure::ModelVisible,
        )),

        _ => None,
    }
}

fn http_config(url: String, headers: &[HttpHeader]) -> Option<StreamableHttpClientTransportConfig> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    for header in headers {
        // ACP supplies complete header values; rmcp's auth_header would prepend `Bearer `.
        config.custom_headers.insert(header.name.parse().ok()?, header.value.parse().ok()?);
    }
    Some(config)
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
            McpTransport::Stdio { command, args, env } => {
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
            McpTransport::Http(config) => {
                assert_eq!(configs[0].name, "http-server");
                assert_eq!(config.transport.uri.as_ref(), "https://example.com/mcp");
                assert!(config.transport.auth_header.is_none());
                assert_eq!(
                    config
                        .transport
                        .custom_headers
                        .iter()
                        .find(|(name, _)| name.as_str() == "authorization")
                        .unwrap()
                        .1
                        .to_str()
                        .unwrap(),
                    "Bearer token123"
                );
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
                McpTransport::Http(config) => {
                    assert!(config.transport.auth_header.is_none());
                    let headers: std::collections::BTreeMap<_, _> = config
                        .transport
                        .custom_headers
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
                        .collect();
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
        let McpTransport::Stdio { args, env, .. } = &configs[0].transport else { panic!("expected stdio") };
        assert!(args.is_empty() && env.is_empty());
        let McpTransport::Http(config) = &configs[1].transport else { panic!("expected http") };
        assert!(config.transport.auth_header.is_none() && config.transport.custom_headers.is_empty());
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
