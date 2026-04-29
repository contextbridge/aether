# SubAgentsMcp

Spawn and orchestrate sub-agents authored in project `.aether/settings.json`.
Sub-agents run in parallel and have access to built-in MCP servers.

**Flag:** `--project-root <path>` (defaults to current directory; `--dir` alias supported)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Project Configuration](#project-configuration)
- [Tools](#tools)
- [Spawning Agents](#spawning-agents)
- [Structured Output](#structured-output)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Project Configuration

Sub-agents are discovered from `.aether/settings.json`:

```json
{
  "agents": [
    {
      "name": "explore",
      "description": "Explores codebases to answer architecture questions",
      "model": "anthropic:claude-sonnet-4-5",
      "agentInvocable": true,
      "prompts": [{ "type": "file", "path": ".aether/prompts/explore.md" }],
      "mcp": [{ "type": "file", "path": ".aether/mcp/explore.json" }],
      "tools": {
        "allow": ["coding__*"],
        "deny": ["coding__write_file", "coding__bash"]
      }
    }
  ]
}
```

Only agents with `agentInvocable: true` are exposed by SubAgentsMcp.

## Tools

| Tool | Description |
|------|-------------|
| `spawn_subagent` | Spawn one or more sub-agents with specific prompts. All agents run in parallel. |

## Spawning Agents

`spawn_subagent` accepts a list of tasks, each with an `agent_name` and `prompt`. All tasks execute in parallel:

```json
{
  "tasks": [
    { "agent_name": "explore", "prompt": "Find all API endpoints in this codebase" },
    { "agent_name": "explore", "prompt": "Document the database schema" }
  ]
}
```

## Structured Output

Sub-agents are encouraged to return structured output with:

- **summary** -- what was accomplished
- **artifacts** -- files read or modified
- **decisions** -- key decisions made
- **next_steps** -- suggested follow-up actions
