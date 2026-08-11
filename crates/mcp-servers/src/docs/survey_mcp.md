MCP server for collecting structured user input via MCP elicitation.

Provides an `ask_user` tool that presents a JSON Schema-defined form to the user and returns their response. This enables agents to gather structured data (confirmations, choices, text input) during a workflow without free-form conversation.

Elicitation is delivered via the MRTR pattern (MCP 2026-07-28): the first `ask_user` call returns an `InputRequiredResult` and the client retries the call with the user's response in `inputResponses`. Clients that do not declare the elicitation capability on protocol 2026-07-28+ receive a tool error.

# Construction

```rust,ignore
use mcp_servers::SurveyMcp;

let server = SurveyMcp::new();
```

# Tools provided

- **`ask_user`** -- Present a message and JSON Schema form to the user. Returns `accepted: true` with the form data, or `accepted: false` if the user declines.
