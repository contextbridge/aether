MCP server for spawning and orchestrating concurrent sub-agents.

Sub-agents are independent agent instances that run in parallel, each with their own tool set and conversation context. This server discovers available agent configurations from the project's `.aether/settings.json` and provides a tool to spawn batches of them.

# Construction

```rust,ignore
use mcp_servers::SubAgentsMcp;

// From project root (loads agent catalog from .aether/settings.json)
let server = SubAgentsMcp::from_project_root("/my/project".into()).unwrap();

// From CLI args
let server = SubAgentsMcp::from_args(vec!["--project-root".into(), ".".into()]).unwrap();
```

# Tools provided

- **`spawn_subagent`** -- Takes a batch of [`SubAgentTask`](crate::subagents::tools::SubAgentTask)s, runs them concurrently, and returns structured outputs with task artifacts and completion status.

# Agent catalog

Agent configurations are discovered from `.aether/settings.json` in the project root. Each agent definition specifies a name, model, system prompt, and available tools.
