Extension trait that registers all built-in MCP server factories onto an [`McpBuilder`](aether_core::mcp::McpBuilder).

Servers are constructed when [`McpBuilder::spawn`](aether_core::mcp::McpBuilder::spawn) runs. Invalid `args` for a server fail `spawn` with [`McpSpawnError::ServerFactory`](aether_core::mcp::McpSpawnError::ServerFactory) instead of falling back to defaults.

# Usage

```rust,ignore
use mcp_servers::McpBuilderExt;
use aether_core::mcp::mcp;
use aether_core::mcp::McpConfig;

let builder = mcp("/my/project")
    .with_agent_deps(deps)
    .with_builtin_servers()
    .with_config(McpConfig::from_json_file("mcp.json").unwrap())
    .unwrap();
```

# See also

- [`CodingMcp`](crate::CodingMcp) -- File I/O, shell, search, and LSP tools
- [`SkillsMcp`](crate::SkillsMcp) -- Skills and slash commands
- [`TasksMcp`](crate::TasksMcp) -- Task management
- [`SubAgentsMcp`](crate::SubAgentsMcp) -- Sub-agent orchestration
- [`ReviewMcp`](crate::ReviewMcp) -- Markdown artifact review workflow
