use agent_client_protocol::schema::v1::{HttpHeader, McpServer};
use mcp_utils::client::{McpServer as RuntimeMcpServer, McpTransport, ToolExposure};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

/// Maps ACP MCP server definitions to internal MCP servers, skipping unsupported transports.
pub fn map_acp_mcp_servers(servers: Vec<McpServer>) -> Vec<RuntimeMcpServer> {
    servers
        .into_iter()
        .filter_map(|s| {
            try_map_mcp_server(s).or_else(|| {
                tracing::warn!("Unsupported ACP MCP server transport, skipping");
                None
            })
        })
        .collect()
}

fn try_map_mcp_server(server: McpServer) -> Option<RuntimeMcpServer> {
    use McpServer::{Http, Sse, Stdio};
    match server {
        Stdio(stdio) => Some(RuntimeMcpServer::new(
            stdio.name,
            McpTransport::Stdio {
                command: stdio.command.to_string_lossy().into_owned(),
                args: stdio.args,
                env: stdio.env.into_iter().map(|e| (e.name, e.value)).collect(),
            },
            ToolExposure::ModelVisible,
        )),

        Http(http) => Some(RuntimeMcpServer::new(
            http.name,
            McpTransport::Http(http_config(http.url, &http.headers).into()),
            ToolExposure::ModelVisible,
        )),

        Sse(sse) => Some(RuntimeMcpServer::new(
            sse.name,
            McpTransport::Http(http_config(sse.url, &sse.headers).into()),
            ToolExposure::ModelVisible,
        )),

        _ => None,
    }
}

fn http_config(url: String, headers: &[HttpHeader]) -> StreamableHttpClientTransportConfig {
    let auth_header = headers.iter().find(|h| h.name.eq_ignore_ascii_case("authorization")).map(|h| h.value.clone());

    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    if let Some(auth) = auth_header {
        // rmcp's `auth_header` wants the bare token; it adds the `Bearer ` prefix itself.
        let token = auth
            .split_once(' ')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map_or(auth.as_str(), |(_, rest)| rest);
        config = config.auth_header(token.to_string());
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1 as acp;

    #[test]
    fn test_map_acp_stdio_server() {
        let server = acp::McpServer::Stdio(
            acp::McpServerStdio::new("my-server", "/usr/bin/server")
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
                assert_eq!(config.transport.auth_header.as_deref(), Some("token123"));
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    fn test_http_auth_header_strips_bearer_case_insensitively() {
        let cases = [
            ("Bearer token123", "token123"),
            ("bearer token123", "token123"),
            ("BEARER token123", "token123"),
            ("bEaReR token123", "token123"),
            // Non-Bearer scheme: pass through verbatim. rmcp will then prefix
            // with "Bearer ", which is the contract for non-bearer auth too.
            ("Token foo", "Token foo"),
            ("token123", "token123"),
        ];

        for (input, expected) in cases {
            let server = acp::McpServer::Http(
                acp::McpServerHttp::new("http-server", "https://example.com/mcp")
                    .headers(vec![acp::HttpHeader::new("Authorization", input)]),
            );
            let configs = map_acp_mcp_servers(vec![server]);
            match &configs[0].transport {
                McpTransport::Http(config) => {
                    assert_eq!(config.transport.auth_header.as_deref(), Some(expected), "input was {input:?}");
                }
                other => panic!("Expected Http, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_map_acp_sse_server() {
        let server = acp::McpServer::Sse(acp::McpServerSse::new("sse-server", "https://example.com/sse"));

        let configs = map_acp_mcp_servers(vec![server]);
        assert_eq!(configs.len(), 1);

        match &configs[0].transport {
            McpTransport::Http(config) => {
                assert_eq!(configs[0].name, "sse-server");
                assert_eq!(config.transport.uri.as_ref(), "https://example.com/sse");
                assert_eq!(config.transport.auth_header, None);
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }
}
