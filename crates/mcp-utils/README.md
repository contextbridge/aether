# aether-mcp-utils

[Model Context Protocol](https://modelcontextprotocol.io/) (MCP) building blocks on top of
[rmcp](https://docs.rs/rmcp): connect to a server, host a server, or aggregate many servers
behind one gateway.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Feature Flags](#feature-flags)
- [Tool kinds](#tool-kinds)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | `client` and `gateway`, including OAuth. Without it, only server hosting remains. | yes |
| `testing` | `testing`: an in-memory `FakeMcpServer`, an OAuth-protected `FakeAuthServer`, and a scripted elicitation host | no |

## Tool kinds

To give a tool an icon or UI treatment in ACP clients, declare its kind on the MCP tool definition (not the result).
For `rmcp`, import `ToolKind` and `tool_kind_meta` from `mcp_utils::tool_kind` and add
`meta = tool_kind_meta(ToolKind::Read)` to the tool's `#[tool(...)]` attribute.

Other MCP implementations can set `"_meta": {"aether-agent.io/tool-kind": "read"}` on the tool returned by
`tools/list`, using a recognized [ACP tool kind](https://agentclientprotocol.com/protocol/v2/tool-calls#reporting).
Aether reports this hint on the call's ACP updates. Missing or unrecognized values use the client's default.
The hint does not affect tool selection or permissions.

## License

MIT
