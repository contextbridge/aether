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

A remote streamable-HTTP server:

```json
{
  "type": "http",
  "url": "https://mcp.example.com",
  "headers": { "Authorization": "Bearer ..." }
}
```
