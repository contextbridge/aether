Controls whether tool calls in [`CodingMcp`](crate::CodingMcp) require user approval before executing.

When a tool call is gated, the server uses MCP elicitation to ask the user for confirmation before proceeding. Read-only tools (`read_file`, `grep`, `find`, `list_files`) are never gated regardless of mode.

# Variants

- **`AlwaysAllow`** (default) -- All tools auto-execute without user approval.
- **`Auto`** -- File writes auto-execute; bash commands that look destructive (`rm`, `git push --force`, redirect operators, etc.) trigger an elicitation prompt.
- **`AlwaysAsk`** -- All write, edit, and bash calls trigger an elicitation prompt.

# See also

- [`CodingMcp::with_permission_mode`](crate::CodingMcp::with_permission_mode) -- Set the mode on a server instance.
