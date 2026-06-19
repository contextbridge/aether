Extension trait that registers all built-in MCP server factories onto an [`McpBuilder`](aether_core::mcp::McpBuilder).

Call [`with_builtin_servers`](McpBuilderExt::with_builtin_servers) to register in-memory server factories for all built-in servers (coding, skills, subagents, survey, plan, tasks) and set workspace roots. After registration, load an `mcp.json` config to control which servers are actually instantiated.

# Usage

```rust,ignore
use mcp_servers::McpBuilderExt;
use aether_core::mcp::mcp;

let builder = mcp()
    .with_builtin_servers("/my/project".into(), "/my/project".as_ref())
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

