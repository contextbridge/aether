Extension trait that registers all built-in MCP server factories onto an [`McpBuilder`](aether_core::mcp::McpBuilder).

Call [`with_builtin_servers`](McpBuilderExt::with_builtin_servers) to register in-memory server factories for all built-in servers (coding, skills, subagents, survey, plan, tasks). Loading `mcp.json` records cloneable server specifications; the concrete servers are created only when [`McpBuilder::spawn`](aether_core::mcp::McpBuilder::spawn) runs.

Install [`AgentDeps`](aether_core::core::AgentDeps) separately with [`with_agent_deps`](aether_core::mcp::McpBuilder::with_agent_deps). At spawn time every factory receives [`RuntimeServices`](aether_core::mcp::RuntimeServices), containing those dependencies, the builder root directory, the live [`McpHandle`](aether_core::mcp::McpHandle), and immutable shell environment entries for the session's deferred-tool gateway. Factory registration does not capture these runtime values.

# Usage

```rust,ignore
use mcp_servers::McpBuilderExt;
use aether_core::mcp::mcp;

let builder = mcp("/my/project")
    .with_agent_deps(deps)
    .with_builtin_servers()
    .from_json_files(&["mcp.json"])
    .unwrap();
```

# See also

- [`CodingMcp`](crate::CodingMcp) -- File I/O, shell, search, and LSP tools
- [`SkillsMcp`](crate::SkillsMcp) -- Skills and slash commands
- [`TasksMcp`](crate::TasksMcp) -- Task management
- [`SubAgentsMcp`](crate::SubAgentsMcp) -- Sub-agent orchestration
- [`SurveyMcp`](crate::SurveyMcp) -- Structured user input
- [`PlanMcp`](crate::PlanMcp) -- Plan review and approval workflow

