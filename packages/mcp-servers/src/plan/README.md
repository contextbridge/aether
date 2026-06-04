<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [PlanMcp](#planmcp)
  - [Tools](#tools)
  - [write_plan](#write_plan)
    - [Input](#input)
    - [Output](#output)
  - [edit_plan](#edit_plan)
    - [Input](#input-1)
    - [Output](#output-1)
  - [submit_plan](#submit_plan)
    - [Input](#input-2)
    - [Output](#output-2)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# PlanMcp

MCP server that exposes plan-specific tools for writing, editing, and submitting implementation plans for user approval/feedback. Agents refer to plans by `planName`; the server maps that name to `<planName>-plan.md` inside the configured plans directory.

By default, built-in Aether wiring stores plans at `${WORKSPACE}/docs/aether/plans`. Standalone `plan-mcp` defaults to `docs/aether/plans` relative to the server process cwd. Configure another directory with `--plans-dir`.

Alternatively, the server can hand submitted plans off to an arbitrary external CLI. Any trailing positional tokens in the `mcp.json` `args` array are treated as the submit command; the resolved absolute plan path is appended as its final argument, and stdout is forwarded verbatim to the agent as `feedback`.

```json
{
  "servers": {
    "plan": {
      "type": "in-memory",
      "args": ["--plans-dir", "docs/plans", "contextbridge", "plan", "--project", "foo"]
    }
  }
}
```

With the above config, calling `submit_plan` with `planName=docs-site-refresh` invokes:

```
contextbridge plan --project foo <absolute-path-to>/docs/plans/docs-site-refresh-plan.md
```

A non-zero exit code surfaces as a tool error; an exit code of `0` returns `{ "approved": false, "feedback": "<stdout>" }` regardless of stdout content — the agent reads the feedback and decides how to proceed.

## Tools

| Tool | Description |
|------|-------------|
| `write_plan` | Write a markdown plan file for a plan name. |
| `edit_plan` | Edit an existing plan file by exact string replacement. |
| `submit_plan` | Submit a named markdown plan file for review and approval. |

## write_plan

### Input

| Field | Type | Description |
|-------|------|-------------|
| `planName` | string | Stable plan identifier, using only letters, numbers, dashes, and underscores. |
| `content` | string | Markdown plan body to write. |

### Output

| Field | Type | Description |
|-------|------|-------------|
| `planName` | string | Plan identifier. |
| `planPath` | string | Absolute path written by the server. |
| `bytesWritten` | number | Number of bytes written. |

## edit_plan

### Input

| Field | Type | Description |
|-------|------|-------------|
| `planName` | string | Stable plan identifier originally passed to `write_plan`. |
| `oldString` | string | Exact string to replace. |
| `newString` | string | Replacement string. |
| `replaceAll` | bool | Whether to replace all occurrences. Defaults to false. |

### Output

| Field | Type | Description |
|-------|------|-------------|
| `planName` | string | Plan identifier. |
| `planPath` | string | Absolute path edited by the server. |
| `replacementsMade` | number | Number of replacements made. |

## submit_plan

### Input

| Field | Type | Description |
|-------|------|-------------|
| `planName` | string | Stable plan identifier originally passed to `write_plan`. |

### Output

| Field | Type | Description |
|-------|------|-------------|
| `approved` | bool | Whether the plan is approved. |
| `feedback` | string \| null | Optional reviewer feedback, typically present on denial. |
