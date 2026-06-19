MCP server for collecting structured user input via the MCP elicitation protocol.

Provides an `ask_user` tool that presents a JSON Schema-defined form to the user and returns their response. This enables agents to gather structured data (confirmations, choices, text input) during a workflow without free-form conversation.

# Construction

```rust,ignore
use mcp_servers::SurveyMcp;

let server = SurveyMcp::new();
```

# Tools provided

- **`ask_user`** -- Present a message and JSON Schema form to the user. Returns `accepted: true` with the form data, or `accepted: false` if the user declines.
