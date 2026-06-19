MCP server for plan review workflows.

Exposes plan-specific file lifecycle tools: `write_plan`, `edit_plan`, and `submit_plan`. Agents address plans by a stable `planName`; the server maps that name to `<planName>-plan.md` inside the configured plans directory. The planner instructions themselves live in a user-customizable skill (e.g. `.aether/skills/plan/SKILL.md`).

Reviews are collected via MCP elicitation: the client receives a form elicitation with `ui: "planReview"` metadata (plus the plan path and markdown body) and returns an approve/deny decision with optional feedback.

# Construction

```rust,ignore
use mcp_servers::PlanMcp;

let server = PlanMcp::new();
```

# Tools provided

- **`write_plan`** -- Writes a markdown plan in the configured plans directory.
- **`edit_plan`** -- Applies a batch of exact-string replacements to an existing plan.
- **`submit_plan`** -- Reads a named plan and returns a structured approval decision.
