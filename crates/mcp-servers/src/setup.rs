use crate::plan::DEFAULT_PLANS_DIR;
use crate::workspace_paths::resolve_path;
use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use aether_core::core::AgentDeps;
use aether_core::mcp::McpBuilder;
use futures::FutureExt;
use mcp_utils::ServiceExt;
use std::path::PathBuf;
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers all built-in in-memory MCP server factories, resolving their
    /// paths against the builder's configured root directory.
    fn with_builtin_servers(self) -> Self;
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

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self) -> Self {
        let context = BuiltinServerContext { root_dir: self.root_dir().to_path_buf(), agent_deps: self.agent_deps() };
        let coding_context = context.clone();
        let skills_context = context.clone();
        let subagents_context = context.clone();
        let plan_context = context.clone();
        let tasks_context = context;

        self.register_in_memory_server(
            "coding",
            Box::new(move |args, _input| {
                let context = coding_context.clone();
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
                    let server = CodingMcp::with_tools(DefaultCodingTools::new())
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
                    SubAgentsMcp::from_args_with_default_project_root(args, &context.root_dir)
                        .expect("Failed to parse SubAgentsMcp args")
                        .with_agent_deps(context.agent_deps)
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
