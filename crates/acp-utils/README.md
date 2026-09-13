# aether-acp-utils

Utilities for the [Agent Client Protocol](https://agentclientprotocol.com/) (ACP), handling notifications, elicitation, and protocol extensions between agents and their host UIs.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Key Types](#key-types)
- [Feature Flags](#feature-flags)
- [WebSocket transport](#websocket-transport)
- [Client ownership and turn completion](#client-ownership-and-turn-completion)
- [Session switching and replay](#session-switching-and-replay)
- [Cancellation and disconnect](#cancellation-and-disconnect)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Key Types

- **`elicitation`** -- Typed conversion between MCP elicitation and native ACP `elicitation/create` messages
- **`SessionUsageParams`** -- Token usage tracking notifications
- **`McpNotification` / `McpRequest`** -- MCP message tunneling over ACP
- **`TokioAcpAgent`** -- Tokio-native ACP transport
- **`AcpClientHandle`** -- Initialized, cloneable client for typed ACP v2 requests
- **`AcpEvent` / `ResumedSession`** -- Live UI events and atomically committed replay snapshots

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | ACP client (for UIs connecting to agents) | yes |
| `websocket` | Message-oriented WebSocket transport | no |
| `testing` | Public in-memory ACP test peers and transport helpers (enables `client`) | no |

## WebSocket transport

Enable the `websocket` feature to use `websocket::WebSocketTransport<S>` with an already-established WebSocket connection.

## License

MIT
