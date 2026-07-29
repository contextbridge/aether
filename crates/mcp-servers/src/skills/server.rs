use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult, PaginatedRequestParams, Prompt,
        PromptArgument, PromptMessage, Role, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, path::Path};
use tokio::sync::RwLock;
use utils::shell_expander::ShellExpander;
use utils::substitution::substitute_parameters;

use super::tools::{
    ListSkillsInput, ListSkillsOutput, LoadSkillsInput, LoadSkillsOutput, SkillFile, SkillListItem, SkillRequest,
};
use crate::{
    error::ServerInitError,
    workspace_paths::{WorkspacePaths, resolve_path},
};
use aether_project::{PromptCatalog, PromptFile, SKILL_FILENAME};
use mcp_utils::display_meta::ToolDisplayMeta;

/// CLI arguments for `SkillsMcp` server
#[derive(Debug, Clone, Parser)]
pub struct SkillsMcpArgs {
    /// Prompt directories to scan directly.
    /// Can be specified multiple times: --dir .aether/skills --dir .claude/rules
    /// On skill name collision, the last directory wins.
    #[arg(long = "dir", required = true)]
    pub dirs: Vec<PathBuf>,

    /// Deprecated: Accepted for backward compatibility but ignored.
    #[arg(long = "notes-dir")]
    pub notes_dir: Option<PathBuf>,
}

impl SkillsMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["skills-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }
}

#[doc = include_str!("../docs/skills_mcp.md")]
#[derive(Clone)]
pub struct SkillsMcp {
    catalog: Arc<RwLock<PromptCatalog>>,
    tool_router: ToolRouter<Self>,
    root_dir: PathBuf,
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
}

impl SkillsMcp {
    pub fn new(prompt_dirs: &[PathBuf]) -> Self {
        let catalog = PromptCatalog::from_dirs(prompt_dirs);

        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            tool_router: Self::tool_router(),
            root_dir: crate::workspace_paths::current_dir(),
        }
    }

    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let parsed_args = SkillsMcpArgs::from_args(args)?;
        Ok(Self::new(&parsed_args.dirs))
    }

    pub fn from_args_with_base_dir(args: Vec<String>, base_dir: &Path) -> Result<Self, ServerInitError> {
        let parsed_args = SkillsMcpArgs::from_args(args)?;
        let dirs = parsed_args.dirs.into_iter().map(|path| resolve_path(base_dir, path)).collect::<Vec<_>>();
        Ok(Self::new(&dirs).with_root_dir(base_dir.to_path_buf()))
    }

    pub fn with_root_dir(mut self, root_dir: PathBuf) -> Self {
        self.root_dir = root_dir;
        self
    }

    fn is_directory_prompt(prompt: &PromptFile) -> bool {
        prompt.path.file_name().is_some_and(|name| name == SKILL_FILENAME)
    }

    async fn resolve_skill_file(
        &self,
        request: &SkillRequest,
    ) -> Result<(PromptFile, PathBuf, String), SkillFileError> {
        let prompt = {
            let catalog = self.catalog.read().await;
            catalog.find(&request.name).cloned()
        }
        .ok_or_else(|| SkillFileError::SkillNotFound(request.name.clone()))?;

        if !prompt.agent_invocable {
            return Err(SkillFileError::NotAgentInvocable(request.name.clone()));
        }

        if Self::is_directory_prompt(&prompt) {
            let skill_dir = prompt.path.parent().ok_or_else(|| SkillFileError::FileNotFound(prompt.path.clone()))?;
            let relative_path = request.path.as_deref().unwrap_or(SKILL_FILENAME);
            let resolved_path = Self::validate_path(skill_dir, relative_path)?;
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

    async fn load_skill_file(&self, request: &SkillRequest, expander: &ShellExpander) -> SkillFile {
        let name = request.name.clone();
        let fallback_path = request.path.clone().unwrap_or_else(|| SKILL_FILENAME.to_string());
        let (prompt, resolved_path, response_path) = match self.resolve_skill_file(request).await {
            Ok(result) => result,
            Err(e) => {
                return SkillFile {
                    name,
                    path: fallback_path,
                    content: None,
                    error: Some(e.to_string()),
                    available_files: Vec::new(),
                };
            }
        };

        let content = match fs::read_to_string(&resolved_path) {
            Ok(content) => content,
            Err(e) => {
                let error = match e.kind() {
                    io::ErrorKind::InvalidData => SkillFileError::InvalidUtf8,
                    _ => SkillFileError::IoError(e),
                };
                return SkillFile {
                    name,
                    path: response_path,
                    content: None,
                    error: Some(error.to_string()),
                    available_files: Vec::new(),
                };
            }
        };

        let expand_cwd = prompt.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let content = expander.expand(&content, &expand_cwd).await;
        let available_files = if Self::is_directory_prompt(&prompt) && response_path == SKILL_FILENAME {
            Self::list_available_files(&expand_cwd)
        } else {
            Vec::new()
        };

        SkillFile { name, path: response_path, content: Some(content), error: None, available_files }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SkillsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().enable_tools().build())
            .with_server_info(Implementation::new("skills-mcp", "0.1.0"))
            .with_instructions(include_str!("./instructions.md"))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let catalog = self.catalog.read().await;
        let prompts = catalog
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
        let catalog = self.catalog.read().await;
        let spec = catalog
            .slash_commands()
            .find(|s| s.name == request.name.as_str())
            .ok_or_else(|| McpError::invalid_params(format!("Prompt '{}' not found", request.name), None))?;

        let body = spec.body.clone();

        let arguments = request.arguments.as_ref().map(|json_map| {
            json_map
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<HashMap<String, String>>()
        });

        let content = substitute_parameters(&body, &arguments);
        let expander = ShellExpander::new();
        let cwd = WorkspacePaths::new(self.root_dir.clone()).root().to_path_buf();
        let content = expander.expand(&content, &cwd).await;
        let messages = vec![PromptMessage::new_text(Role::User, content)];

        Ok(rmcp::model::GetPromptResult::new(messages).with_description(spec.description.clone()).into())
    }
}

#[tool_router]
impl SkillsMcp {
    #[doc = include_str!("tools/list_skills/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn list_skills(&self, request: Parameters<ListSkillsInput>) -> Result<Json<ListSkillsOutput>, String> {
        let Parameters(_input) = request;
        let catalog = self.catalog.read().await;
        let mut skills: Vec<_> = catalog
            .skills()
            .map(|skill| SkillListItem {
                name: skill.name.clone(),
                description: skill.description.clone(),
                tags: skill.tags.clone(),
            })
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        let count = skills.len();

        Ok(Json(ListSkillsOutput {
            status: "success".to_string(),
            skills,
            count,
            message: format!("Found {count} skills"),
            meta: Some(ToolDisplayMeta::new("Skills", format!("{count} skills")).into()),
        }))
    }

    #[doc = include_str!("tools/get_skills/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn get_skills(&self, request: Parameters<LoadSkillsInput>) -> Result<Json<LoadSkillsOutput>, String> {
        let Parameters(args) = request;
        let expander = ShellExpander::new();
        let mut files = Vec::with_capacity(args.requests.len());
        for req in args.requests {
            files.push(self.load_skill_file(&req, &expander).await);
        }

        Ok(Json(LoadSkillsOutput { files }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ServerHandler;
    use tempfile::TempDir;

    fn create_skill(prompt_dir: &Path, name: &str, content: &str, aux_files: &[(&str, &str)]) {
        let skill_dir = prompt_dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join(SKILL_FILENAME), content).unwrap();
        for (path, content) in aux_files {
            let full_path = skill_dir.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full_path, content).unwrap();
        }
    }

    fn create_flat_prompt(prompt_dir: &Path, name: &str, content: &str) {
        fs::create_dir_all(prompt_dir).unwrap();
        fs::write(prompt_dir.join(format!("{name}.md")), content).unwrap();
    }

    async fn load(server: &SkillsMcp, name: &str, path: Option<&str>) -> SkillFile {
        let expander = ShellExpander::new();
        let request = SkillRequest { name: name.to_string(), path: path.map(str::to_string) };
        server.load_skill_file(&request, &expander).await
    }

    #[test]
    fn skills_args_require_at_least_one_dir() {
        let parsed = SkillsMcpArgs::from_args(vec!["--notes-dir".into(), ".aether/notes".into()]);
        assert!(parsed.is_err());
    }

    #[test]
    fn skills_args_parse_repeated_dirs() {
        let parsed = SkillsMcpArgs::from_args(vec![
            "--dir".into(),
            ".aether/skills".into(),
            "--dir".into(),
            ".claude/rules".into(),
        ])
        .unwrap();

        assert_eq!(parsed.dirs, vec![PathBuf::from(".aether/skills"), PathBuf::from(".claude/rules")]);
    }

    #[test]
    fn skills_args_accepts_deprecated_notes_dir() {
        let parsed = SkillsMcpArgs::from_args(vec![
            "--dir".into(),
            ".aether/skills".into(),
            "--notes-dir".into(),
            ".aether/notes".into(),
        ])
        .unwrap();

        assert_eq!(parsed.dirs, vec![PathBuf::from(".aether/skills")]);
        assert_eq!(parsed.notes_dir, Some(PathBuf::from(".aether/notes")));
    }

    #[test]
    fn from_args_with_base_dir_resolves_relative_dirs() {
        let workspace = PathBuf::from("/workspace");
        let server = SkillsMcp::from_args_with_base_dir(
            vec!["--dir".into(), ".aether/skills".into(), "--notes-dir".into(), ".aether/notes".into()],
            &workspace,
        )
        .unwrap();

        assert_eq!(server.root_dir, workspace);
    }

    #[tokio::test]
    async fn test_list_skills_only_returns_agent_invocable_sorted_by_name() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "zeta",
            "---\ndescription: Zeta\nagent-invocable: true\ntags:\n  - systems\n---\n# Zeta",
            &[],
        );
        create_skill(
            temp_dir.path(),
            "user-only",
            "---\ndescription: User only\nuser-invocable: true\nagent-invocable: false\n---\n# User only",
            &[],
        );
        create_flat_prompt(
            temp_dir.path(),
            "flat-agent",
            "---\nname: alpha-flat\ndescription: Flat skill\nagent-invocable: true\ntags:\n  - flat\n---\n# Flat",
        );
        create_flat_prompt(
            temp_dir.path(),
            "rule-only",
            "---\ndescription: Rule only\nagent-invocable: false\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n# Rule",
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let Json(output) = server.list_skills(Parameters(ListSkillsInput::default())).await.unwrap();

        assert_eq!(output.status, "success");
        assert_eq!(output.count, 2);
        assert_eq!(output.message, "Found 2 skills");

        let names: Vec<_> = output.skills.iter().map(|skill| skill.name.clone()).collect();
        assert_eq!(names, vec!["alpha-flat".to_string(), "zeta".to_string()]);

        let alpha = output.skills.iter().find(|skill| skill.name == "alpha-flat").unwrap();
        assert_eq!(alpha.description, "Flat skill");
        assert_eq!(alpha.tags, vec!["flat".to_string()]);

        let zeta = output.skills.iter().find(|skill| skill.name == "zeta").unwrap();
        assert_eq!(zeta.description, "Zeta");
        assert_eq!(zeta.tags, vec!["systems".to_string()]);

        let meta = output.meta.expect("list_skills should return display metadata");
        assert_eq!(meta.display.title, "Skills");
        assert_eq!(meta.display.value, "2 skills");
    }

    #[test]
    fn test_get_info_instructions_reference_list_skills_and_do_not_embed_skill_names() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "unique-instructions-skill-a",
            "---\ndescription: A\nagent-invocable: true\n---\n# A",
            &[],
        );
        create_skill(
            temp_dir.path(),
            "unique-instructions-skill-b",
            "---\ndescription: B\nagent-invocable: true\n---\n# B",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let info = ServerHandler::get_info(&server);
        let instructions = info.instructions.expect("skills server should provide instructions");

        assert!(instructions.contains("list_skills"));
        assert!(!instructions.contains("Complete List of Available Skills"));
        assert!(!instructions.contains("unique-instructions-skill-a"));
        assert!(!instructions.contains("unique-instructions-skill-b"));
    }

    #[tokio::test]
    async fn test_get_skills_rejects_non_agent_invocable_directory_prompt() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "user-only",
            "---\ndescription: User-only prompt\nuser-invocable: true\nagent-invocable: false\n---\n# User only",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "user-only", None).await;

        assert!(result.content.is_none());
        assert!(result.error.as_deref().unwrap().contains("not agent-invocable"));
    }

    #[tokio::test]
    async fn test_get_skills_rejects_non_agent_invocable_flat_prompt() {
        let temp_dir = TempDir::new().unwrap();
        create_flat_prompt(
            temp_dir.path(),
            "rule-only",
            "---\ndescription: Rule-only prompt\nagent-invocable: false\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n# Rule",
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "rule-only", None).await;

        assert!(result.content.is_none());
        assert!(result.error.as_deref().unwrap().contains("not agent-invocable"));
    }

    #[tokio::test]
    async fn test_load_skill_file_root() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\n# Test\n\nContent here.",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", None).await;

        assert_eq!(result.name, "test-skill");
        assert_eq!(result.path, "SKILL.md");
        assert!(result.content.is_some());
        assert!(result.content.unwrap().contains("Content here."));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_load_skill_file_by_frontmatter_name() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "dir-name",
            "---\nname: custom-name\ndescription: Test\nagent-invocable: true\n---\n# Test\n\nContent here.",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "custom-name", None).await;

        assert_eq!(result.name, "custom-name");
        assert_eq!(result.path, "SKILL.md");
        assert!(result.content.is_some());
        assert!(result.content.unwrap().contains("Content here."));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_load_flat_prompt_by_frontmatter_name() {
        let temp_dir = TempDir::new().unwrap();
        create_flat_prompt(
            temp_dir.path(),
            "rule-file",
            "---\nname: rust-rules\ndescription: Rust rules\nagent-invocable: true\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nUse Rust conventions.",
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "rust-rules", None).await;

        assert_eq!(result.name, "rust-rules");
        assert_eq!(result.path, "rule-file.md");
        let content = result.content.expect("flat prompt content should exist");
        assert!(content.contains("Use Rust conventions."));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_load_flat_prompt_file() {
        let temp_dir = TempDir::new().unwrap();
        create_flat_prompt(
            temp_dir.path(),
            "rust-rules",
            "---\ndescription: Rust rules\nagent-invocable: true\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nUse Rust conventions.",
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "rust-rules", None).await;

        assert_eq!(result.path, "rust-rules.md");
        let content = result.content.expect("flat prompt content should exist");
        assert!(content.contains("Use Rust conventions."));
        assert!(result.available_files.is_empty());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_flat_prompt_rejects_relative_path_reads() {
        let temp_dir = TempDir::new().unwrap();
        create_flat_prompt(
            temp_dir.path(),
            "rust-rules",
            "---\ndescription: Rust rules\nagent-invocable: true\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nUse Rust conventions.",
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "rust-rules", Some("details.md")).await;

        assert!(result.content.is_none());
        assert!(result.available_files.is_empty());
        assert!(result.error.as_deref().unwrap().contains("does not support relative paths"));
    }

    #[tokio::test]
    async fn test_load_skill_file_auxiliary() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\n# Test",
            &[("traits.md", "# Traits content")],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some("traits.md")).await;

        assert_eq!(result.path, "traits.md");
        assert_eq!(result.content.unwrap(), "# Traits content");
        assert!(result.available_files.is_empty());
    }

    #[tokio::test]
    async fn test_load_skill_file_nested() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\n# Test",
            &[("references/REF.md", "# Reference")],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some("references/REF.md")).await;

        assert_eq!(result.path, "references/REF.md");
        assert_eq!(result.content.unwrap(), "# Reference");
    }

    #[tokio::test]
    async fn test_reject_absolute_path() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(temp_dir.path(), "test-skill", "---\ndescription: Test\nagent-invocable: true\n---\n# Test", &[]);

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some("/etc/passwd")).await;

        assert!(result.error.unwrap().contains("Absolute paths"));
    }

    #[tokio::test]
    async fn test_reject_traversal() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(temp_dir.path(), "test-skill", "---\ndescription: Test\nagent-invocable: true\n---\n# Test", &[]);

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some("../other-skill/SKILL.md")).await;

        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn test_reject_directory() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(temp_dir.path(), "test-skill", "---\ndescription: Test\nagent-invocable: true\n---\n# Test", &[]);

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some(".")).await;

        assert!(result.error.unwrap().contains("directory"));
    }

    #[tokio::test]
    async fn test_available_files() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\n# Test",
            &[
                ("traits.md", "# Traits"),
                ("error-handling.md", "# Errors"),
                ("references/REF.md", "# Ref"),
                (".hidden", "should be ignored"),
            ],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", None).await;

        assert_eq!(result.available_files.len(), 3);
        assert!(result.available_files.contains(&"error-handling.md".to_string()));
        assert!(result.available_files.contains(&"references/REF.md".to_string()));
        assert!(result.available_files.contains(&"traits.md".to_string()));
        assert!(!result.available_files.contains(&"SKILL.md".to_string()));
    }

    #[tokio::test]
    async fn test_skill_not_found() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join("skills")).unwrap();

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "nonexistent", None).await;

        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(temp_dir.path(), "test-skill", "---\ndescription: Test\nagent-invocable: true\n---\n# Test", &[]);

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", Some("nonexistent.md")).await;

        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_batch_requests() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "rust",
            "---\ndescription: Rust skill\nagent-invocable: true\n---\n# Rust\n\nSee [traits](./traits.md).",
            &[("traits.md", "# Traits")],
        );
        create_skill(
            temp_dir.path(),
            "python",
            "---\ndescription: Python skill\nagent-invocable: true\n---\n# Python",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let expander = ShellExpander::new();
        let requests = vec![
            SkillRequest { name: "rust".to_string(), path: None },
            SkillRequest { name: "rust".to_string(), path: Some("traits.md".to_string()) },
            SkillRequest { name: "python".to_string(), path: None },
        ];

        let mut files = Vec::with_capacity(requests.len());
        for req in requests {
            files.push(server.load_skill_file(&req, &expander).await);
        }

        assert_eq!(files.len(), 3);

        assert_eq!(files[0].name, "rust");
        assert_eq!(files[0].path, "SKILL.md");
        assert!(files[0].content.is_some());
        assert!(files[0].available_files.contains(&"traits.md".to_string()));

        assert_eq!(files[1].name, "rust");
        assert_eq!(files[1].path, "traits.md");
        assert_eq!(files[1].content.as_deref(), Some("# Traits"));
        assert!(files[1].available_files.is_empty());

        assert_eq!(files[2].name, "python");
        assert_eq!(files[2].path, "SKILL.md");
        assert!(files[2].content.is_some());
    }

    #[tokio::test]
    async fn test_mixed_success_failure() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(temp_dir.path(), "exists", "---\ndescription: Exists\nagent-invocable: true\n---\n# Exists", &[]);

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let expander = ShellExpander::new();
        let requests = vec![
            SkillRequest { name: "exists".to_string(), path: None },
            SkillRequest { name: "nonexistent".to_string(), path: None },
            SkillRequest { name: "exists".to_string(), path: Some("missing.md".to_string()) },
        ];

        let mut files = Vec::with_capacity(requests.len());
        for req in requests {
            files.push(server.load_skill_file(&req, &expander).await);
        }

        assert_eq!(files.len(), 3);

        assert!(files[0].content.is_some());
        assert!(files[0].error.is_none());

        assert!(files[1].content.is_none());
        assert!(files[1].error.as_ref().unwrap().contains("not found"));

        assert!(files[2].content.is_none());
        assert!(files[2].error.as_ref().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_multi_dir_last_wins() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        create_skill(dir_a.path(), "rust", "---\ndescription: Rust A\nagent-invocable: true\n---\n# From A", &[]);
        create_skill(dir_b.path(), "rust", "---\ndescription: Rust B\nagent-invocable: true\n---\n# From B", &[]);

        let server = SkillsMcp::new(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);
        let result = load(&server, "rust", None).await;

        assert!(result.content.as_ref().unwrap().contains("From B"));
    }

    #[tokio::test]
    async fn test_multi_dir_union() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        create_skill(dir_a.path(), "rust", "---\ndescription: Rust\nagent-invocable: true\n---\n# Rust", &[]);
        create_skill(dir_b.path(), "python", "---\ndescription: Python\nagent-invocable: true\n---\n# Python", &[]);

        let server = SkillsMcp::new(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);

        let rust = load(&server, "rust", None).await;
        assert!(rust.content.is_some());
        assert!(rust.error.is_none());

        let python = load(&server, "python", None).await;
        assert!(python.content.is_some());
        assert!(python.error.is_none());
    }

    #[tokio::test]
    async fn test_multi_dir_missing_skills_subdir() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        create_skill(dir_b.path(), "rust", "---\ndescription: Rust\nagent-invocable: true\n---\n# Rust", &[]);

        let server = SkillsMcp::new(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);
        let result = load(&server, "rust", None).await;

        assert!(result.content.is_some());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_skill_file_expands_shell_interp() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\nvalue: !`echo ok`",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", None).await;

        let content = result.content.expect("content should be present");
        assert!(content.contains("value: ok"), "expected expanded marker in: {content}");
        assert!(!content.contains("!`"), "expected no literal marker in: {content}");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_skill_file_propagates_shell_failure() {
        let temp_dir = TempDir::new().unwrap();
        create_skill(
            temp_dir.path(),
            "test-skill",
            "---\ndescription: Test\nagent-invocable: true\n---\n!`exit 1`",
            &[],
        );

        let server = SkillsMcp::new(&[temp_dir.path().to_path_buf()]);
        let result = load(&server, "test-skill", None).await;

        let content = result.content.expect("content should be present");
        assert!(!content.contains("!`"), "failed command marker should be removed");
        assert!(result.error.is_none());
    }
}
