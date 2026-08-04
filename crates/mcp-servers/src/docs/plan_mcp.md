MCP server for plan review workflows.

Exposes plan-specific file lifecycle tools: `write_plan`, `edit_plan`, and `submit_plan`. Agents address plans by a stable `planName`; the server maps that name to `<planName>-plan.md` inside the configured plans directory. The planner instructions themselves live in a user-customizable skill (e.g. `.aether/skills/plan/SKILL.md`).

Reviews use MCP 2026-07-28 multi-round-trip requests (MRTR). Native review returns an intermediate `input_required` result containing a keyed form and plan-review `_meta`; the client retries with the original arguments, exact opaque state, and the `ElicitResult`. The final structured result preserves `approved` and optional feedback. The external submit command override remains a one-round process flow. Interactive review requires a client advertising form elicitation; older peers may still use the non-interactive plan tools.

# Construction

```rust,ignore
use mcp_servers::PlanMcp;

let server = PlanMcp::new();
```

# Tools provided

- **`write_plan`** -- Writes a markdown plan in the configured plans directory.
- **`edit_plan`** -- Applies a batch of exact-string replacements to an existing plan.
- **`submit_plan`** -- Reads a named plan and returns a structured approval decision.
