# SubAgentsMcp

Spawn and orchestrate sub-agents authored in project `.aether/settings.json`.
Sub-agents in a batch run concurrently and have access to built-in MCP servers. Foreground execution is the default. Background execution returns one protocol-level MCP Task, requires MCP Tasks support, and cancelling the batch task stops all remaining agents.

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
      "prompts": [".aether/prompts/explore.md"],
      "mcps": [".aether/mcp/explore.json"],
      "tools": {
        "allow": ["coding__*"],
        "deny": ["coding__write_file", "coding__bash"]
      }
    }
  ]
}
```

Only agents with `agentInvocable: true` are exposed by SubAgentsMcp. Top-level `prompts` and `mcps` act as defaults for agents that do not define their own agent-local `prompts` or `mcps`; agent-local values override the top-level defaults.

## Tools

| Tool | Description |
|------|-------------|
| `spawn_subagent` | Spawn one or more sub-agents concurrently. Waits for the whole batch by default; `runInBackground: true` returns an MCP Task. |

## Spawning Agents

`spawn_subagent` accepts a list of tasks, each with an `agentName` and `prompt`. All children execute concurrently and results preserve input order. By default the call waits for the entire batch, allowing the parent to use the results in its next step. Set `runInBackground: true` for independent work that may complete later:

```json
{
  "tasks": [
    {"agentName": "codebase-explorer", "prompt": "Find all API endpoints"},
    {"agentName": "rust-code-monkey", "prompt": "Write tests for auth module"}
  ],
  "runInBackground": true
}
```

## Structured Output

Foreground calls return `results`, `successCount`, `errorCount`, and display metadata directly. Background calls return an MCP Task whose completed payload contains the same data. Background mode requires the client to support MCP Tasks.

Each item in `results` includes the child agent's structured output. Child agents are instructed to return:

- **summary** -- what was accomplished
- **artifacts** -- files read or modified
- **decisions** -- key decisions made
- **nextSteps** -- suggested follow-up actions
