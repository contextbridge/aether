Extension trait that registers all built-in MCP server factories onto an [`McpBuilder`](aether_core::mcp::McpBuilder).


# Usage

```rust,ignore
use mcp_servers::McpBuilderExt;
use aether_core::mcp::mcp;
use mcp_utils::config::McpConfig;

let builder = mcp("/my/project")
    .with_agent_deps(deps)
    .with_builtin_servers()
    .with_config(McpConfig::from_json_files(&["mcp.json"]).unwrap())
    .unwrap();
```

# See also

- [`CodingMcp`](crate::CodingMcp) -- File I/O, shell, search, and LSP tools
- [`SkillsMcp`](crate::SkillsMcp) -- Skills and slash commands
- [`TasksMcp`](crate::TasksMcp) -- Task management
- [`SubAgentsMcp`](crate::SubAgentsMcp) -- Sub-agent orchestration
- [`ReviewMcp`](crate::ReviewMcp) -- Markdown artifact review workflow
