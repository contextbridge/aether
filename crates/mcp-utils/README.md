# aether-mcp-utils

Utilities for the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP), providing transport, status tracking, and client management for MCP servers.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Feature Flags](#feature-flags)
- [Aggregating MCP servers](#aggregating-mcp-servers)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | Streamable HTTP MCP client, server management, tool catalog, and `AggregateServer` | yes |
| `oauth` | OAuth authorization and credential storage for HTTP servers | yes |
| `stdio` | Child-process (stdio) MCP servers | yes |
| `ipc` | Unix socket transport for the progressive-discovery tool gateway | yes |
| `testing` | Fake MCP servers and an HTTP test server | no |

For a lean HTTP-only client, use `default-features = false, features = ["client"]`.

## Aggregating MCP servers

`McpManager` connects to the servers in an `mcp.json`-style config, and `AggregateServer` serves all of their tools and prompts as a single MCP server under `server__name`. `AggregateServer` is transport-agnostic, so a stateless HTTP gateway is a few lines of `axum`:

```rust,no_run
use mcp_utils::aggregate::AggregateServer;
use mcp_utils::client::{McpConfig, McpManager, RuntimeMcpServer, Vars};
use mcp_utils::rmcp::model::ClientCapabilities;
use mcp_utils::rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `headers` values such as "Bearer ${GITHUB_TOKEN}" expand from the environment.
    let servers = McpConfig::from_json_file("gateway.json")?
        .into_servers(&Vars::new())?
        .into_iter()
        .map(RuntimeMcpServer::try_from)
        .collect::<Result<Vec<_>, _>>()?;

    // Nobody is around to answer elicitations, so advertise no client capabilities.
    let manager = McpManager::new().with_client_capabilities(ClientCapabilities::default());
    let handle = manager.handle();
    let _manager = manager.spawn(servers).await?;

    let server = AggregateServer::new(handle);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_allowed_hosts(["gateway.example.com"]),
    );
    let app = axum::Router::new().route_service("/mcp", service);
    axum::serve(tokio::net::TcpListener::bind("0.0.0.0:8080").await?, app).await?;
    Ok(())
}
```

`McpManager::with_reconnect` retries HTTP servers that fail to connect, and `mcp_utils::testing::HttpTestServer` serves any handler over stateless HTTP for tests.

## License

MIT
