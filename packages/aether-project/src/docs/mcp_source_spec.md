MCP config source — either a file path string or an inline server definition.

Point at a JSON file of servers:

```json
".aether/mcp.json"
```

Reference a file with options:

```json
{ "type": "file", "path": ".aether/mcp.json", "optional": true }
```

Or define servers inline:

```json
{
  "type": "inline",
  "servers": {
    "github": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"]
    }
  }
}
```
