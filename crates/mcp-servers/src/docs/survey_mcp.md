MCP server for collecting structured user input via the MCP elicitation protocol.

Provides an `ask_user` tool that presents a JSON Schema-defined form to the user through an MCP 2026-07-28 multi-round-trip request (MRTR). The first call returns `resultType: "input_required"` with a keyed elicitation; the retry echoes the original arguments and opaque request state and returns the existing `accepted`/`data` structured output. Decline and cancel produce `accepted: false`. Interactive calls require a client advertising form elicitation.

# Construction

```rust,ignore
use mcp_servers::SurveyMcp;

let server = SurveyMcp::new();
```

# Tools provided

- **`ask_user`** -- Present a message and JSON Schema form to the user. Returns `accepted: true` with the form data, or `accepted: false` if the user declines.
