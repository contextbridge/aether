# aether-mcp-utils

Utilities for the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP), providing transport, status tracking, and client management for MCP servers.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Key Types](#key-types)
- [Feature Flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Key Types

- **`InMemoryTransport`** -- In-process MCP transport for running servers without subprocesses
- **`McpServerStatus`** -- Tracks server connection state (`Connected`, `Failed`, `NeedsOAuth`)
- **`ToolDisplayMeta` / `ToolResultMeta`** -- Metadata for rendering tool calls and results in UIs
- **`McpClient::fulfill_mrtr_input_requests`** -- Sequentially fulfills supported embedded MRTR elicitation requests through the host UI.

## Protocol compatibility

The client negotiates MCP protocol `2026-07-28` first through `ClientLifecycleMode::Auto`, falling back to older revisions and legacy `initialize` when necessary. Ordinary tools and prompts continue to work with older third-party servers. Third-party interactive workflows are supported only after negotiating `2026-07-28` and using SEP-2322 MRTR; legacy direct `elicitation/create` tool flows and the non-standard `-32042` URL-elicitation error are not compatibility paths. Aether currently fulfills only embedded elicitation requests and rejects MRTR sampling or roots requests before displaying any input.

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | MCP client with OAuth, server management, and tool proxying | yes |

## License

MIT
