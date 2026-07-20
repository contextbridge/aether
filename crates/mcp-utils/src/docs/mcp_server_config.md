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
