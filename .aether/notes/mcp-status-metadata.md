---
topic: mcp-status-metadata
tags:
- mcp
- coding-style
- aether
updated: "2026-05-07"
---
- For MCP server status metadata, prefer a simple boolean `proxy` flag over a `Direct/Proxied` grouping enum. The enum was considered over-engineered and not useful for this codebase.
