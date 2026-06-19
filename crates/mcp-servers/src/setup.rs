use crate::plan::DEFAULT_PLANS_DIR;
use crate::workspace_paths;
use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, PlanMcp, SkillsMcp, SubAgentsMcp, SurveyMcp, TasksMcp};
use aether_auth::OAuthCredentialStorage;
use aether_core::mcp::McpBuilder;
use futures::FutureExt;
use mcp_utils::ServiceExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, warn};

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    /// Registers all built-in in-memory MCP server factories and workspace roots.
    fn with_builtin_servers(
        self,
        cwd: PathBuf,
        roots_path: &Path,
        oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    ) -> Self;
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(
        self,
        _cwd: PathBuf,
        roots_path: &Path,
        oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    ) -> Self {
        let workspace_root = roots_path.to_path_buf();
        let coding_cwd = workspace_root.clone();
        let plan_cwd = workspace_root.clone();
        let subagents_cwd = workspace_root.clone();
        let skills_cwd = workspace_root.clone();
        let tasks_cwd = workspace_root.clone();
        self.register_in_memory_server(
            "coding",
            Box::new(move |args, _input| {
                let project_path = coding_cwd.clone();
                async move {
                    let parsed = match CodingMcpArgs::from_args(args) {
                        Ok(args) => args,
                        Err(e) => {
                            warn!("CodingMcp args parse failed: {e}, using defaults");
                            CodingMcpArgs::default()
                        }
                    };
                    let CodingMcpArgs { permission_mode, mut rules_dirs, disable_lsp, .. } = parsed;
                    rules_dirs =
                        rules_dirs.into_iter().map(|path| workspace_paths::resolve_path(&project_path, path)).collect();
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
            Box::new(move |args, _input| {
                let cwd = skills_cwd.clone();
                async move {
                    SkillsMcp::from_args_with_workspace_root(args, &cwd)
                        .expect("Failed to parse SkillsMcp args")
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "subagents",
            Box::new(move |args, _input| {
                let store = oauth_credential_store.clone();
                let cwd = subagents_cwd.clone();
                async move {
                    let mut mcp = SubAgentsMcp::from_args_with_default_project_root(args, &cwd)
                        .expect("Failed to parse SubAgentsMcp args");
                    if let Some(store) = store {
                        mcp = mcp.with_oauth_credential_store(store);
                    }
                    mcp.into_dyn()
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
                let default_plans_dir = plan_cwd.join(DEFAULT_PLANS_DIR);
                let workspace_root = plan_cwd.clone();
                async move {
                    PlanMcp::from_args_with_workspace_root(args, default_plans_dir, &workspace_root)
                        .expect("Failed to parse PlanMcp args")
                        .into_dyn()
                }
                .boxed()
            }),
        )
        .register_in_memory_server(
            "tasks",
            Box::new(move |args, _input| {
                let cwd = tasks_cwd.clone();
                async move {
                    TasksMcp::from_args_with_workspace_root(args, &cwd)
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
