Controls whether tool calls in [`CodingMcp`](crate::CodingMcp) require user approval before executing.

When a tool call is gated, the server uses MCP 2026-07-28 multi-round-trip requests (MRTR) to ask the user for confirmation before proceeding. The initial `tools/call` returns `resultType: "input_required"`; the client retries with the original arguments, the exact opaque `requestState`, and the keyed elicitation response. No filesystem or process side effect runs before an integrity-checked `allow` response. Interactive permission requests require a modern client advertising form elicitation; older protocol peers can use only non-interactive tools.

# Variants

- **`AlwaysAllow`** (default) -- All tools auto-execute without user approval.
- **`Auto`** -- File writes auto-execute; bash commands that look destructive (`rm`, `git push --force`, redirect operators, etc.) trigger an elicitation prompt.
- **`AlwaysAsk`** -- All write, edit, and bash calls trigger an elicitation prompt.

# See also

- [`CodingMcp::with_permission_mode`](crate::CodingMcp::with_permission_mode) -- Set the mode on a server instance.
