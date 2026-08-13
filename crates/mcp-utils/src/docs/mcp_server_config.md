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

When `oauth` is omitted, Aether uses dynamic client registration and a random
loopback callback port. When present, Aether binds the configured callback port
and advertises `http://localhost:<callbackPort>/`, which must match the redirect
URI registered for the OAuth client.

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
