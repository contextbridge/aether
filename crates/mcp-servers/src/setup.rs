use crate::coding::tools::bash::BashEnvironment;
use crate::plan::DEFAULT_PLANS_DIR;
use crate::workspace_paths::resolve_path;
use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use aether_core::mcp::{McpBuilder, RuntimeServices};
use futures::FutureExt;
use mcp_utils::ServiceExt;
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers built-in in-memory MCP factories. Servers are constructed at
    /// spawn time with the runtime's root directory, dependencies, and MCP handle.
    fn with_builtin_servers(self) -> Self;
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self) -> Self {
        self.register_in_memory_server(
            "coding",
            Box::new(|spec, services| {
                async move {
                    let parsed = match CodingMcpArgs::from_args(spec.args) {
                        Ok(args) => args,
                        Err(e) => {
                            warn!("CodingMcp args parse failed: {e}, using defaults");
                            CodingMcpArgs::default()
                        }
                    };
                    let CodingMcpArgs { permission_mode, mut rules_dirs, disable_lsp, root_dir: arg_root_dir } = parsed;
                    let root_dir = arg_root_dir
                        .map_or_else(|| services.root_dir.clone(), |path| resolve_path(&services.root_dir, path));
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
                    let server = if disable_lsp { server } else { server.with_lsp(root_dir) };
                    server.into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "skills",
            Box::new(|spec, services| {
                async move {
                    SkillsMcp::from_args_with_base_dir(spec.args, &services.root_dir)
                        .map_err(|error| {
                            warn!("Failed to parse SkillsMcp args: {error}, using defaults");
                            error
                        })
                        .unwrap_or_else(|_| SkillsMcp::new(&[]).with_root_dir(services.root_dir.clone()))
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "subagents",
            Box::new(|spec, services| {
                async move {
                    let agent_deps = services.agent_deps.clone();
                    SubAgentsMcp::embedded_from_args(spec.args, &services.root_dir, agent_deps.clone())
                        .map_err(|error| {
                            warn!("Failed to parse SubAgentsMcp args: {error}, using defaults");
                            error
                        })
                        .unwrap_or_else(|_| SubAgentsMcp::embedded(services.root_dir.clone(), agent_deps))
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "survey",
            Box::new(|_spec, _services| async move { SurveyMcp::new().into_dyn() }.boxed()),
        )
        .register_in_memory_server(
            "plan",
            Box::new(|spec, services| {
                async move {
                    let default_plans_dir = services.root_dir.join(DEFAULT_PLANS_DIR);
                    PlanMcp::from_args_with_base_dir(spec.args, default_plans_dir, &services.root_dir)
                        .map_err(|error| {
                            warn!("Failed to parse PlanMcp args: {error}, using defaults");
                            error
                        })
                        .unwrap_or_else(|_| PlanMcp::new())
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "tasks",
            Box::new(|spec, services| {
                async move {
                    TasksMcp::from_args_with_base_dir(spec.args, &services.root_dir)
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to parse TasksMcp args: {e}, using defaults");
                            TasksMcp::new()
                        })
                        .into_dyn()
                }
                .boxed()
            }),
        )
    }
}

fn coding_bash_environment(services: &RuntimeServices) -> BashEnvironment {
    services
        .shell_environment
        .iter()
        .fold(BashEnvironment::new().with_current_exe_dir_on_path(), |environment, (name, value)| {
            environment.with_var(name, value)
        })
}
