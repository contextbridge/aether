MCP server for spawning and orchestrating concurrent sub-agents.

Sub-agents are independent agent instances that run in parallel, each with their own tool set and conversation context. Embedded servers use the active runtime agent catalog supplied by Aether. Standalone servers can discover agent configurations from a project's `.aether/settings.json`.

# Construction

```rust,ignore
use clap::Parser;
use mcp_servers::{SubAgentsMcp, SubAgentsMcpArgs};

// Embedded: Aether supplies the complete runtime dependencies
let server = SubAgentsMcp::new("/my/project".into(), deps);

// Embedded, from CLI args resolved against a base directory
let args = SubAgentsMcpArgs::try_parse_from(["--project-root", "sub"])?;
let server = SubAgentsMcp::from_args(args.clone(), "/my/project".as_ref(), deps);

// Standalone: discovers agents from the project's settings
let server = SubAgentsMcp::standalone(args, "/my/project".as_ref())?;
```

# Tools provided

- **`spawn_subagent`** -- Takes a batch of [`SubAgentTask`](crate::subagents::tools::SubAgentTask)s, runs them concurrently, and returns structured outputs with task artifacts and completion status.

# Agent catalog

Delegation targets come from the [`AgentRegistry`](aether_core::core::AgentRegistry) in the [`AgentDeps`](aether_core::core::AgentDeps) passed to [`SubAgentsMcp::new`](crate::SubAgentsMcp::new). Aether resolves the registry once at startup and threads it down through the deps, so SDK settings and other explicit settings sources stay authoritative at every delegation level — including sub-agents that themselves delegate. Embedded servers never reload agent definitions from the workspace.

Standalone construction through [`SubAgentsMcp::standalone`](crate::SubAgentsMcp::standalone) or CLI arguments has no host to inherit from, so it discovers agents from `.aether/settings.json` in the project root and seeds its own deps with them. Each definition specifies a name, model, system prompt, and available tools.
