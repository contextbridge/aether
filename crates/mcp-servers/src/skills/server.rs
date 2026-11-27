use crate::server_config;
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::skill_catalog::SkillCatalog,
    model::{
        GetPromptRequestParams, GetPromptResponse, GetSkillRequestParams, GetSkillResult, ListPromptsResult,
        ListSkillsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage, ReadResourceRequestParams,
        ReadResourceResponse, Role, ServerCapabilities, ServerConfig, Skill,
    },
    service::RequestContext,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::spawn_blocking;
use utils::shell_expander::expand_shell;

use super::substitution::substitute_parameters;
use crate::workspace_paths::resolve_path;
use aether_project::PromptCatalog;

/// CLI arguments for `SkillsMcp` server
#[derive(Debug, Clone, Parser)]
#[command(name = "skills", no_binary_name = true)]
pub struct SkillsMcpArgs {
    /// Skill directories to serve.
    /// Can be specified multiple times: --dir .aether/skills --dir .claude/skills
    /// On name collision, the last directory wins.
    #[arg(long = "dir", required = true)]
    pub dirs: Vec<PathBuf>,
}

#[doc = include_str!("../docs/skills_mcp.md")]
#[derive(Clone)]
pub struct SkillsMcp {
    prompts: Arc<PromptCatalog>,
    catalogs: Arc<[SkillCatalog]>,
    root_dir: PathBuf,
}

impl SkillsMcp {
    pub fn new(root_dir: impl Into<PathBuf>, skill_dirs: &[PathBuf]) -> Self {
        let catalogs = skill_dirs.iter().rev().filter_map(|dir| {
            SkillCatalog::from_dir(dir).inspect_err(|error| tracing::warn!(%error, "Skipping skill directory")).ok()
        });

        Self {
            prompts: Arc::new(PromptCatalog::from_dirs(skill_dirs)),
            catalogs: catalogs.collect(),
            root_dir: root_dir.into(),
        }
    }

    pub fn from_args(args: SkillsMcpArgs, base_dir: &Path) -> Self {
        let dirs = args.dirs.into_iter().map(|path| resolve_path(base_dir, path)).collect::<Vec<_>>();
        Self::new(base_dir, &dirs)
    }

    fn skills(&self) -> Vec<Skill> {
        let mut served = HashSet::new();
        let skills = self.catalogs.iter().flat_map(SkillCatalog::skills);
        let mut skills: Vec<Skill> = skills.filter(|skill| served.insert(skill.uri.clone())).collect();
        skills.retain(is_agent_invocable);
        skills.sort_by(|a, b| a.uri.cmp(&b.uri));
        skills
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl ServerHandler for SkillsMcp {
    fn get_info(&self) -> ServerConfig {
        server_config(
            "skills-mcp",
            ServerCapabilities::builder().enable_prompts().enable_resources().enable_skills().build(),
            include_str!("./instructions.md"),
        )
    }

    async fn list_skills(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, McpError> {
        let catalogs = Arc::clone(&self.catalogs);
        let reload = spawn_blocking(move || {
            for catalog in catalogs.iter() {
                if let Err(error) = catalog.reload() {
                    tracing::warn!(%error, "Failed to reload skill directory");
                }
            }
        });
        reload.await.map_err(|error| McpError::internal_error(format!("Reloading skills failed: {error}"), None))?;

        Ok(ListSkillsResult::with_all_items(self.skills()))
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, McpError> {
        self.catalogs
            .iter()
            .find_map(|catalog| catalog.get_skill(&request.uri).ok())
            .filter(|found| is_agent_invocable(&found.skill))
            .ok_or_else(|| McpError::invalid_params(format!("unknown skill {}", request.uri), None))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        self.catalogs
            .iter()
            .find_map(|catalog| catalog.read_resource(&request.uri))
            .map(Into::into)
            .ok_or_else(|| McpError::resource_not_found(format!("unknown resource {}", request.uri), None))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = self
            .prompts
            .slash_commands()
            .map(|s| {
                let arguments = s.argument_hint.as_ref().map(|hint| {
                    vec![PromptArgument::new("ARGUMENTS").with_description(hint.clone()).with_required(false)]
                });

                Prompt::new(s.name.clone(), Some(s.description.clone()), arguments)
            })
            .collect();

        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let spec = self
            .prompts
            .slash_commands()
            .find(|s| s.name == request.name.as_str())
            .ok_or_else(|| McpError::invalid_params(format!("Prompt '{}' not found", request.name), None))?;

        let arguments: HashMap<String, String> = request
            .arguments
            .iter()
            .flatten()
            .filter_map(|(name, value)| value.as_str().map(|value| (name.clone(), value.to_string())))
            .collect();

        let content = substitute_parameters(&expand_shell(&spec.body, &self.root_dir).await, &arguments);
        let messages = vec![PromptMessage::new_text(Role::User, content)];

        Ok(rmcp::model::GetPromptResult::new(messages).with_description(spec.description.clone()).into())
    }
}

fn is_agent_invocable(skill: &Skill) -> bool {
    skill.frontmatter.extra.get("agent-invocable") != Some(&Value::Bool(false))
}
