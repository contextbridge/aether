A single MCP server. The `type` field selects the transport; `stdio` is the
default and may be omitted.

A local stdio server launched as a subprocess:

```json
{
  "type": "stdio",
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-github"],
  "env": { "GITHUB_TOKEN": "ghp_..." }
}
```

A remote streamable-HTTP server using a pre-registered public OAuth client:

```json
{
  "type": "http",
  "url": "https://mcp.slack.com/mcp",
  "oauth": {
    "clientId": "1601185624273.8899143856786",
    "callbackPort": 3118
  }
}
```

When a remote HTTP server returns an OAuth challenge, Aether uses its first-party
Client ID Metadata Document at
`https://aether-agent.io/oauth/client-metadata.json` and listens on
`127.0.0.1:3118`, advertising the exact redirect URI `http://localhost:3118/`.
If port 3118 is occupied, authentication fails until the port is available.

The `oauth` object is optional. Set `clientMetadataUrl` for a custom CIMD, or
`clientId` for a pre-registered public client; a configured `clientId` takes
priority. `callbackPort` defaults to 3118 and must exactly match the client's
registered redirect URI. If the authorization server does not advertise CIMD,
Aether falls back to deprecated Dynamic Client Registration. An explicit
`Authorization` header bypasses OAuth entirely.

A remote server using a bearer token:

```json
{
  "type": "http",
  "url": "https://mcp.example.com",
  "headers": { "Authorization": "Bearer ..." }
}
```

Set `"proxy": true` to expose every tool on this server through Aether's
shared `proxy__call_tool`. For selective proxying, set `proxy` to an object with
`include` and `exclude` lists. Entries match either an exact MCP-local tool name
or a prefix ending in a trailing `*`:

```json
{
  "type": "in-memory",
  "proxy": {
    "include": ["*"],
    "exclude": ["bash", "lsp_*"]
  }
}
```

An omitted `include` list includes every tool. `exclude` is applied afterward
and wins when both lists match. Included tools are omitted from direct tool
definitions and written to proxy discovery files. Excluded or non-included tools
remain available through their normal `server__tool` names and cannot be called
through `proxy__call_tool`. Patterns use names local to the MCP server, without
the `server__` prefix. A selectively proxied server's instructions remain
available for its directly exposed tools.

Aether records both direct and proxied tools in the per-agent `ToolCatalog`.
The configured `proxy` rules and agent tool filter are evaluated once when the
catalog entry is built. All model definitions, proxy discovery files, server
instructions, status rows, and execution authorization are then projected from
that same cached state. Agent filter name patterns match canonical
`server__tool` names; a denied proxied tool is neither written for discovery nor
executable through `proxy__call_tool`.
