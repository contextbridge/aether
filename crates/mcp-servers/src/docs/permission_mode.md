Controls whether tool calls in [`CodingMcp`](crate::CodingMcp) require user approval before executing.

When a tool call is gated, the server asks the user for confirmation via MCP elicitation delivered through the MRTR pattern (MCP 2026-07-28): the call returns an `InputRequiredResult` with an allow/deny form, and the client retries the call with the decision — the gated tool only executes on an allowed retry. Clients that cannot elicit (no elicitation capability, or protocol older than 2026-07-28) get a tool error for gated calls.

Which tools are gated is derived from each tool's `destructive_hint` annotation, so tools declared destructive (`write_file`, `edit_file`, `bash`, `lsp_rename`, and any future ones) are covered automatically. Read-only tools are never gated regardless of mode.

# Variants

- **`AlwaysAllow`** (default) -- All tools auto-execute without user approval.
- **`Auto`** -- File writes auto-execute; bash commands that look destructive (`rm`, `git push --force`, redirect operators, etc.) trigger an elicitation prompt.
- **`AlwaysAsk`** -- Every destructive-annotated tool call triggers an elicitation prompt.

# See also

- [`CodingMcp::with_permission_mode`](crate::CodingMcp::with_permission_mode) -- Set the mode on a server instance.
