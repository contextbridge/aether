Extension trait that registers all built-in MCP server factories onto an [`McpBuilder`](aether_core::mcp::McpBuilder).

Call [`with_builtin_servers`](McpBuilderExt::with_builtin_servers) to register in-memory server factories for all built-in servers (coding, skills, subagents, survey, plan, tasks). Their paths are resolved against the builder's root directory (set via [`mcp`](aether_core::mcp::mcp)). After registration, load an `mcp.json` config to control which servers are actually instantiated.

The supplied [`AgentDeps`](aether_core::core::AgentDeps) are installed on the builder and captured by the embedded servers, so registration cannot be ordered incorrectly.

# Usage

```rust,ignore
use mcp_servers::McpBuilderExt;
use aether_core::mcp::mcp;

let builder = mcp("/my/project")
    .with_builtin_servers(deps)
    .from_json_files(&["mcp.json"])
    .await
    .unwrap();
```

# See also

- [`CodingMcp`](crate::CodingMcp) -- File I/O, shell, search, and LSP tools
- [`SkillsMcp`](crate::SkillsMcp) -- Skills and slash commands
- [`TasksMcp`](crate::TasksMcp) -- Task management
- [`SubAgentsMcp`](crate::SubAgentsMcp) -- Sub-agent orchestration
- [`SurveyMcp`](crate::SurveyMcp) -- Structured user input
- [`PlanMcp`](crate::PlanMcp) -- Plan review and approval workflow

