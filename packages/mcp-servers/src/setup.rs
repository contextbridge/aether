use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use aether_core::mcp::McpBuilder;
use futures::FutureExt;
use mcp_utils::ServiceExt;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers all built-in in-memory MCP server factories and workspace roots.
    fn with_builtin_servers(self, cwd: PathBuf, roots_path: &Path) -> Self;
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self, cwd: PathBuf, roots_path: &Path) -> Self {
        self.register_in_memory_server(
            "coding",
            Box::new(move |args, _input| {
                let project_path = cwd.clone();
                async move {
                    let parsed = match CodingMcpArgs::from_args(args) {
                        Ok(args) => args,
                        Err(e) => {
                            warn!("CodingMcp args parse failed: {e}, using defaults");
                            CodingMcpArgs::default()
                        }
                    };
                    let CodingMcpArgs { permission_mode, rules_dirs, disable_lsp, .. } = parsed;
                    debug!(
                        "CodingMcp created, disable_lsp={}, permission_mode={:?}, rules_dirs={}",
                        disable_lsp,
                        permission_mode,
                        rules_dirs.len()
                    );
                    let server = CodingMcp::with_tools(DefaultCodingTools::new())
                        .with_rules_dirs(rules_dirs)
                        .with_root_dir(project_path.clone())
                        .with_permission_mode(permission_mode);
                    let server = if disable_lsp { server } else { server.with_lsp(project_path) };
                    server.into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "skills",
            Box::new(|args, _input| {
                async move { SkillsMcp::from_args(args).expect("Failed to parse SkillsMcp args").into_dyn() }.boxed()
            }),
        )
        .register_in_memory_server(
            "subagents",
            Box::new(|args, _input| {
                async move { SubAgentsMcp::from_args(args).expect("Failed to parse SubAgentsMcp args").into_dyn() }
                    .boxed()
            }),
        )
        .register_in_memory_server(
            "survey",
            Box::new(|_args, _input| async move { SurveyMcp::new().into_dyn() }.boxed()),
        )
        .register_in_memory_server(
            "plan",
            Box::new(|args, _input| {
                async move { PlanMcp::from_args(args).expect("Failed to parse PlanMcp args").into_dyn() }.boxed()
            }),
        )
        .register_in_memory_server(
            "tasks",
            Box::new(move |args, _input| {
                async move {
                    TasksMcp::from_args(args)
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to parse TasksMcp args: {e}, using defaults");
                            TasksMcp::new()
                        })
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .with_roots(vec![roots_path.to_path_buf()])
    }
}
