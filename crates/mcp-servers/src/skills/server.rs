use crate::server_config;
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::schema_for_output,
        wrapper::{Json, Parameters},
    },
    model::{
        GetPromptRequestParams, GetPromptResponse, ListPromptsResult, PaginatedRequestParams, Prompt, PromptArgument,
        PromptMessage, Role, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, path::Path};
use utils::shell_expander::expand_shell;

use super::substitution::substitute_parameters;
use super::tools::{
    ListSkillsInput, ListSkillsOutput, LoadSkillsInput, LoadSkillsOutput, SkillFile, SkillListItem, SkillRequest,
};
use crate::Displayed;
use crate::workspace_paths::resolve_path;
use aether_project::{PromptCatalog, PromptFile, SKILL_FILENAME};
use utils::display_meta::ToolDisplayMeta;

/// CLI arguments for `SkillsMcp` server
#[derive(Debug, Clone, Parser)]
#[command(name = "skills", no_binary_name = true)]
pub struct SkillsMcpArgs {
    /// Prompt directories to scan directly.
    /// Can be specified multiple times: --dir .aether/skills --dir .claude/rules
    /// On skill name collision, the last directory wins.
    #[arg(long = "dir", required = true)]
    pub dirs: Vec<PathBuf>,
}

#[doc = include_str!("../docs/skills_mcp.md")]
#[derive(Clone)]
pub struct SkillsMcp {
    catalog: Arc<PromptCatalog>,
    tool_router: ToolRouter<Self>,
    root_dir: PathBuf,
}

impl SkillsMcp {
    pub fn new(root_dir: impl Into<PathBuf>, prompt_dirs: &[PathBuf]) -> Self {
        let catalog = PromptCatalog::from_dirs(prompt_dirs);

        Self { catalog: Arc::new(catalog), tool_router: Self::tool_router(), root_dir: root_dir.into() }
    }

    pub fn from_args(args: SkillsMcpArgs, base_dir: &Path) -> Self {
        let dirs = args.dirs.into_iter().map(|path| resolve_path(base_dir, path)).collect::<Vec<_>>();
        Self::new(base_dir, &dirs)
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for SkillsMcp {
    fn get_info(&self) -> ServerConfig {
        server_config(
            "skills-mcp",
            ServerCapabilities::builder().enable_prompts().enable_tools().build(),
            include_str!("./instructions.md"),
        )
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = self
            .catalog
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
            .catalog
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

#[tool_router]
impl SkillsMcp {
    #[doc = include_str!("tools/list_skills/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<ListSkillsOutput>()
    )]
    pub async fn list_skills(&self, Parameters(_): Parameters<ListSkillsInput>) -> Displayed<ListSkillsOutput> {
        let mut skills: Vec<_> = self
            .catalog
            .skills()
            .map(|skill| SkillListItem {
                name: skill.name.clone(),
                description: skill.description.clone(),
                tags: skill.tags.clone(),
            })
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        let meta = ToolDisplayMeta::new("Skills", format!("{} skills", skills.len()));

        Displayed::new(ListSkillsOutput { skills }, meta)
    }

    #[doc = include_str!("tools/get_skills/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn get_skills(&self, Parameters(args): Parameters<LoadSkillsInput>) -> Json<LoadSkillsOutput> {
        let mut files = Vec::with_capacity(args.requests.len());
        for request in &args.requests {
            files.push(self.load_skill_file(request).await);
        }

        Json(LoadSkillsOutput { files })
    }
}

impl SkillsMcp {
    async fn load_skill_file(&self, request: &SkillRequest) -> SkillFile {
        self.try_load_skill_file(request).await.unwrap_or_else(|error| SkillFile {
            name: request.name.clone(),
            path: request.path.clone().unwrap_or_else(|| SKILL_FILENAME.to_string()),
            content: None,
            error: Some(error.to_string()),
            available_files: Vec::new(),
        })
    }

    async fn try_load_skill_file(&self, request: &SkillRequest) -> Result<SkillFile, SkillFileError> {
        let catalog = Arc::clone(&self.catalog);
        let blocking_request = request.clone();
        let read = tokio::task::spawn_blocking(move || read_skill_file(&catalog, &blocking_request)).await??;
        let content = expand_shell(&read.content, &read.skill_dir).await;

        Ok(SkillFile {
            name: request.name.clone(),
            path: read.path,
            content: Some(content),
            error: None,
            available_files: read.available_files,
        })
    }
}

/// A skill file as read from disk, before its shell markers are expanded.
struct SkillFileRead {
    path: String,
    content: String,
    skill_dir: PathBuf,
    available_files: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum SkillFileError {
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    #[error("Skill '{0}' is not agent-invocable")]
    NotAgentInvocable(String),
    #[error("Prompt '{0}' is a flat markdown file and does not support relative paths")]
    FlatFileDoesNotSupportRelativePaths(String),
    #[error("Absolute paths are not allowed")]
    AbsolutePath,
    #[error("Path traversal (..) is not allowed")]
    TraversalAttempt,
    #[error("Resolved path escapes skill directory")]
    EscapeAttempt,
    #[error("Path is a directory, not a file")]
    IsDirectory,
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("File content is not valid UTF-8")]
    InvalidUtf8,
    #[error("Reading the skill file failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

fn read_skill_file(catalog: &PromptCatalog, request: &SkillRequest) -> Result<SkillFileRead, SkillFileError> {
    let (prompt, resolved_path, response_path) = resolve_skill_file(catalog, request)?;
    let content = fs::read_to_string(&resolved_path).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => SkillFileError::InvalidUtf8,
        _ => SkillFileError::IoError(e),
    })?;

    let skill_dir = prompt.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let available_files = if is_directory_prompt(&prompt) && response_path == SKILL_FILENAME {
        list_available_files(&skill_dir)
    } else {
        Vec::new()
    };

    Ok(SkillFileRead { path: response_path, content, skill_dir, available_files })
}

fn resolve_skill_file(
    catalog: &PromptCatalog,
    request: &SkillRequest,
) -> Result<(PromptFile, PathBuf, String), SkillFileError> {
    let prompt =
        catalog.find(&request.name).cloned().ok_or_else(|| SkillFileError::SkillNotFound(request.name.clone()))?;

    if !prompt.agent_invocable {
        return Err(SkillFileError::NotAgentInvocable(request.name.clone()));
    }

    if is_directory_prompt(&prompt) {
        let skill_dir = prompt.path.parent().ok_or_else(|| SkillFileError::FileNotFound(prompt.path.clone()))?;
        let relative_path = request.path.as_deref().unwrap_or(SKILL_FILENAME);
        let resolved_path = validate_path(skill_dir, relative_path)?;
        Ok((prompt, resolved_path, relative_path.to_string()))
    } else {
        if request.path.is_some() {
            return Err(SkillFileError::FlatFileDoesNotSupportRelativePaths(prompt.name.clone()));
        }

        let response_path = prompt
            .path
            .file_name()
            .map_or_else(|| format!("{}.md", prompt.name), |name| name.to_string_lossy().to_string());
        let resolved_path = prompt.path.clone();
        Ok((prompt, resolved_path, response_path))
    }
}

fn is_directory_prompt(prompt: &PromptFile) -> bool {
    prompt.path.file_name().is_some_and(|name| name == SKILL_FILENAME)
}

fn validate_path(skill_dir: &Path, relative_path: &str) -> Result<PathBuf, SkillFileError> {
    if Path::new(relative_path).is_absolute() {
        return Err(SkillFileError::AbsolutePath);
    }

    if relative_path.contains("..") {
        return Err(SkillFileError::TraversalAttempt);
    }

    let canonical_skill_dir = skill_dir.canonicalize().map_err(SkillFileError::IoError)?;
    let file_path = skill_dir.join(relative_path);
    let canonical_file_path = file_path.canonicalize().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => SkillFileError::FileNotFound(file_path),
        _ => SkillFileError::IoError(e),
    })?;

    if !canonical_file_path.starts_with(&canonical_skill_dir) {
        return Err(SkillFileError::EscapeAttempt);
    }

    if canonical_file_path.is_dir() {
        return Err(SkillFileError::IsDirectory);
    }

    Ok(canonical_file_path)
}

fn list_available_files(skill_dir: &Path) -> Vec<String> {
    fn collect_files(dir: &Path, base: &Path, files: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.') {
                    continue;
                }

                let path = entry.path();
                if path.is_dir() {
                    collect_files(&path, base, files);
                } else if path.is_file() {
                    if name_str == SKILL_FILENAME {
                        continue;
                    }

                    if let Ok(relative) = path.strip_prefix(base) {
                        files.push(relative.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    let mut files = Vec::new();
    if skill_dir.is_dir() {
        collect_files(skill_dir, skill_dir, &mut files);
        files.sort();
    }
    files
}
