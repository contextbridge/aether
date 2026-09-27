# aether-acp-utils

Utilities for the [Agent Client Protocol](https://agentclientprotocol.com/) (ACP), handling notifications, elicitation, and protocol extensions between agents and their host UIs.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Key Types](#key-types)
- [Feature Flags](#feature-flags)
- [WebSocket transport](#websocket-transport)
- [WebAssembly](#webassembly)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Key Types

- **`elicitation`** -- Typed conversion between MCP elicitation and native ACP `elicitation/create` messages
- **`SessionUsageParams`** -- Token usage tracking notifications
- **`McpNotification` / `McpRequest`** -- MCP message tunneling over ACP
- Agent subprocesses -- use upstream `agent_client_protocol::{AcpAgent, AcpAgentConfig, Stdio}`
- **`AcpClientHandle`** -- Initialized, cloneable client for typed ACP v2 requests
- **`AcpEvent`** -- Ordered notifications, including replay updates sent before the resume response; `AcpEvent::session_id` tells hosts which session an event concerns
- **`conversation::Conversation`** -- Host-independent reduction of one session's events (`apply_event`) and prompt lifecycle into ordered items (messages, thoughts, tool calls, notices) and turn state; shared by `wisp` and the browser client

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | ACP client and conversation model (for UIs connecting to agents) | yes |
| `websocket` | Message-oriented WebSocket transport | no |
| `testing` | Public in-memory ACP test peers and transport helpers (enables `client`) | no |

## WebSocket transport

Enable the `websocket` feature to use `websocket::WebSocketTransport<S>` with an already-established WebSocket connection.

`examples/fake_agent_server.rs` serves the `testing` crate's fake agent over this transport, choosing a scenario by request path. `just fake-agent` runs it on `aether server`'s port, and `just wasm-test` runs the browser client's tests against it.


## License

MIT
