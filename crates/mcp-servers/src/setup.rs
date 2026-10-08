use crate::coding::tools::bash::BashEnvironment;
use crate::workspace_paths::resolve_path;
use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, ReviewMcp, SkillsMcp, SubAgentsMcp, TasksMcp};
use aether_core::mcp::{AETHER_MCP_IPC_SOCKET, McpBuilder, RuntimeServices};
use mcp_utils::server::McpServer;
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers built-in in-memory MCP factories. Servers are constructed at
    fn with_builtin_servers(self) -> Self;
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self) -> Self {
        self.register_in_memory_server("coding", |config, services| {
            let parsed = match CodingMcpArgs::from_args(config.args) {
                Ok(args) => args,
                Err(e) => {
                    warn!("CodingMcp args parse failed: {e}, using defaults");
                    CodingMcpArgs::default()
                }
            };
            let CodingMcpArgs { permission_mode, mut rules_dirs, disable_lsp, root_dir: arg_root_dir } = parsed;
            let root_dir =
                arg_root_dir.map_or_else(|| services.root_dir.clone(), |path| resolve_path(&services.root_dir, path));
            rules_dirs = rules_dirs.into_iter().map(|path| resolve_path(&root_dir, path)).collect();
            debug!(
                "CodingMcp created, disable_lsp={}, permission_mode={:?}, rules_dirs={}",
                disable_lsp,
                permission_mode,
                rules_dirs.len()
            );
            let tools = DefaultCodingTools::new().with_bash_environment(coding_bash_environment(&services));
            let server = CodingMcp::with_tools(tools)
                .with_rules_dirs(rules_dirs)
                .with_root_dir(root_dir.clone())
                .with_permission_mode(permission_mode);
            McpServer::new(if disable_lsp { server } else { server.with_lsp(root_dir) })
        })
        .register_in_memory_server("skills", |config, services| {
            let server = SkillsMcp::from_args_with_base_dir(config.args, &services.root_dir)
                .map_err(|error| {
                    warn!("Failed to parse SkillsMcp args: {error}, using defaults");
                    error
                })
                .unwrap_or_else(|_| SkillsMcp::new(&[]).with_root_dir(services.root_dir.clone()));
            McpServer::new(server)
        })
        .register_in_memory_server("subagents", |config, services| {
            let agent_deps = services.agent_deps.clone();
            let server = SubAgentsMcp::embedded_from_args(config.args, &services.root_dir, agent_deps.clone())
                .map_err(|error| {
                    warn!("Failed to parse SubAgentsMcp args: {error}, using defaults");
                    error
                })
                .unwrap_or_else(|_| SubAgentsMcp::embedded(services.root_dir.clone(), agent_deps));
            McpServer::new(server)
        })
        .register_in_memory_server("review", |config, services| {
            let server = ReviewMcp::from_args_with_base_dir(config.args, &services.root_dir)
                .inspect_err(|error| {
                    warn!("Failed to parse ReviewMcp args: {error}, using workspace root");
                })
                .unwrap_or_else(|_| {
                    ReviewMcp::from_args_with_base_dir(Vec::new(), &services.root_dir)
                        .expect("empty review MCP arguments are valid")
                });
            McpServer::new(server)
        })
        .register_in_memory_server("tasks", |config, services| {
            let server = TasksMcp::from_args_with_base_dir(config.args, &services.root_dir).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse TasksMcp args: {e}, using defaults");
                TasksMcp::new()
            });
            McpServer::new(server)
        })
    }
}

fn coding_bash_environment(services: &RuntimeServices) -> BashEnvironment {
    let environment = BashEnvironment::new().with_current_exe_dir_on_path();
    match &services.deferred_tools_socket {
        Some(socket) => environment.with_var(AETHER_MCP_IPC_SOCKET, socket.to_string_lossy()),
        None => environment,
    }
}
