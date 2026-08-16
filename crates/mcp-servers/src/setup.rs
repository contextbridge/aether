use crate::coding::tools::bash::BashEnvironment;
use crate::plan::DEFAULT_PLANS_DIR;
use crate::workspace_paths::resolve_path;
use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use aether_core::core::AgentDeps;
use aether_core::mcp::{DeferredToolGateway, McpBuilder};
use futures::FutureExt;
use mcp_utils::ServiceExt;
use mcp_utils::client::ServerFactory;
use mcp_utils::tool_gateway::ServerDescription;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers all built-in in-memory MCP server factories, resolving their
    /// paths against the builder's configured root directory.
    fn with_builtin_servers(self, agent_deps: AgentDeps, bash_environment: BashEnvironment) -> Self;
}

#[derive(Clone)]
struct BuiltinServerContext {
    root_dir: PathBuf,
    agent_deps: AgentDeps,
}

impl BuiltinServerContext {
    fn resolve(&self, path: PathBuf) -> PathBuf {
        resolve_path(&self.root_dir, path)
    }
}

pub fn aether_bash_environment() -> BashEnvironment {
    let executable = std::env::current_exe().ok();
    BashEnvironment::for_aether_executable(executable.as_deref())
}

pub fn assemble_progressive_discovery(
    mut builder: McpBuilder,
    bash_environment: &BashEnvironment,
) -> (McpBuilder, Option<DeferredToolGateway>) {
    if !builder.has_deferred_tools() {
        return (builder, None);
    }

    match DeferredToolGateway::bind() {
        Ok(gateway) => {
            bash_environment.extend(gateway.environment());
            builder = builder.with_progressive_discovery_instructions(Arc::new(progressive_discovery_instructions));
            (builder, Some(gateway))
        }
        Err(error) => {
            warn!(%error, "Failed to bind deferred tool gateway; progressive discovery disabled");
            (builder, None)
        }
    }
}

pub fn progressive_discovery_instructions(servers: &[ServerDescription]) -> String {
    use std::fmt::Write;

    let mut instructions = [
        "Deferred MCP tools are available through the shell via `aether mcp <server> <tool> [arguments]`.",
        "Progressively discover them with `aether mcp --help`, `aether mcp <server> --help`, and `aether mcp <server> <tool> --help`.",
        "Arguments may be supplied as `key=value`, `--key value`, `--args '<JSON object>'`, or a JSON object on stdin.",
        "Tool results are JSON. Commands can be composed with ordinary shell pipelines and `jq`.",
        "",
        "## Deferred MCP Servers",
        "",
    ]
    .join("\n");

    for server in servers {
        let _ = writeln!(instructions, "- **{}**: {}", server.name, server.description);
    }
    instructions
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self, agent_deps: AgentDeps, bash_environment: BashEnvironment) -> Self {
        let builder = self;
        let context =
            BuiltinServerContext { root_dir: builder.root_dir().to_path_buf(), agent_deps: agent_deps.clone() };
        let coding_context = context.clone();
        let skills_context = context.clone();
        let subagents_context = context.clone();
        let plan_context = context.clone();
        let tasks_context = context;

        builder
            .with_agent_deps(agent_deps)
            .register_in_memory_server("coding", coding_server_factory(coding_context, bash_environment))
            .register_in_memory_server(
                "skills",
                Box::new(move |args, _input| {
                    let context = skills_context.clone();
                    async move {
                        SkillsMcp::from_args_with_base_dir(args, &context.root_dir)
                            .expect("Failed to parse SkillsMcp args")
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .register_in_memory_server(
                "subagents",
                Box::new(move |args, _input| {
                    let context = subagents_context.clone();
                    async move {
                        SubAgentsMcp::embedded_from_args(args, &context.root_dir, context.agent_deps)
                            .expect("Failed to parse SubAgentsMcp args")
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .register_in_memory_server(
                "survey",
                Box::new(|_args, _input| async move { SurveyMcp::new().into_dyn() }.boxed()),
            )
            .register_in_memory_server(
                "plan",
                Box::new(move |args, _input| {
                    let default_plans_dir = plan_context.root_dir.join(DEFAULT_PLANS_DIR);
                    let root_dir = plan_context.root_dir.clone();
                    async move {
                        PlanMcp::from_args_with_base_dir(args, default_plans_dir, &root_dir)
                            .expect("Failed to parse PlanMcp args")
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .register_in_memory_server(
                "tasks",
                Box::new(move |args, _input| {
                    let context = tasks_context.clone();
                    async move {
                        TasksMcp::from_args_with_base_dir(args, &context.root_dir)
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

fn coding_server_factory(context: BuiltinServerContext, bash_environment: BashEnvironment) -> ServerFactory {
    Box::new(move |args, _input| {
        let context = context.clone();
        let bash_environment = bash_environment.clone();
        async move {
            let parsed = match CodingMcpArgs::from_args(args) {
                Ok(args) => args,
                Err(e) => {
                    warn!("CodingMcp args parse failed: {e}, using defaults");
                    CodingMcpArgs::default()
                }
            };
            let CodingMcpArgs { permission_mode, mut rules_dirs, disable_lsp, root_dir: arg_root_dir } = parsed;
            let root_dir = arg_root_dir.map_or_else(|| context.root_dir.clone(), |path| context.resolve(path));
            rules_dirs = rules_dirs.into_iter().map(|path| resolve_path(&root_dir, path)).collect();
            debug!(
                "CodingMcp created, disable_lsp={}, permission_mode={:?}, rules_dirs={}",
                disable_lsp,
                permission_mode,
                rules_dirs.len()
            );
            let server = CodingMcp::with_tools(DefaultCodingTools::new().with_bash_environment(bash_environment))
                .with_rules_dirs(rules_dirs)
                .with_root_dir(root_dir.clone())
                .with_permission_mode(permission_mode);
            let server = if disable_lsp { server } else { server.with_lsp(root_dir) };
            server.into_dyn()
        }
        .boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::mcp::mcp;

    #[test]
    fn progressive_discovery_is_not_assembled_without_deferred_tools() {
        let (builder, gateway) = assemble_progressive_discovery(mcp("/workspace"), &BashEnvironment::default());

        assert!(!builder.has_deferred_tools());
        assert!(gateway.is_none());
    }
}
