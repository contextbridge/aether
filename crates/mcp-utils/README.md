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
- **Progressive Tool Discovery** -- Keeps deferred tools out of the initial model context and exposes them on demand through `aether mcp <server> <tool>`

The tool gateway is an MCP server over a Unix socket in a private `0700` runtime directory. The owning runtime exports `AETHER_MCP_IPC_SOCKET` to its coding shell; standard MCP `tools/list`, `tools/call`, framing, and cancellation are handled by `rmcp`.

- **`ToolDisplayMeta` / `ToolResultMeta`** -- Metadata for rendering tool calls and results in UIs

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | MCP client with OAuth, server management, and progressive tool discovery | yes |

## License

MIT
