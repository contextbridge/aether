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
- **`AcpEvent`** -- Ordered notifications, including replay updates sent before the resume response
- **`conversation::Conversation`** -- Host-independent reduction of one session's updates, sub-agent progress and prompt lifecycle into ordered items (messages, tool calls, notices) and turn state; shared by `wisp` and the browser client

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | ACP client (for UIs connecting to agents) | yes |
| `websocket` | Message-oriented WebSocket transport | no |
| `testing` | Public in-memory ACP test peers and transport helpers (enables `client`) | no |

## WebSocket transport

Enable the `websocket` feature to use `websocket::WebSocketTransport<S>` with an already-established WebSocket connection.

## WebAssembly

The wire types (`meta`, `config_meta`, `config_option_id`, `notifications`), `content`, `conversation` and the `client` module also build for `wasm32-unknown-unknown`, where ACP's `wasm_js` feature supplies randomness and the connection is driven by the browser event loop instead of Tokio. The `aether-acp-wasm` crate builds a browser client on top of them.

These integrations with native-only crates are unavailable on wasm:

- `content::{map_acp_to_content_blocks, map_user_content_block}` -- conversions to and from `aether-llm` content blocks
- `elicitation` -- conversions to and from MCP (`rmcp`) elicitation
- `notifications::SessionUsageParams` -- carries `aether-llm` usage events

The `websocket` feature is the native `tokio-tungstenite` transport; wasm builds use the browser transport in `aether-acp-wasm` instead.

## License

MIT
