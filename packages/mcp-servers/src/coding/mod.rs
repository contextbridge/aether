use aether_project::PromptCatalog;
use clap::Parser;
use mcp_utils::client::{InMemoryServerConfig, McpConfig, McpServerConfig};
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        CreateElicitationRequestParams, ElicitationSchema, EnumSchema, Implementation, ProgressNotificationParam,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::{
    fs::{canonicalize, read_to_string, try_exists},
    sync::{Mutex, RwLock},
};

pub mod default_tools;
pub mod error;
pub mod prompt_rule_matcher;
pub mod tools;
pub mod tools_trait;

pub use default_tools::DefaultCodingTools;
pub use tools_trait::CodingTools;

use crate::lsp::tools::check_errors::{
    LspDiagnosticsInput, LspDiagnosticsOutput, LspDiagnosticsRequest, execute_lsp_diagnostics,
};
use crate::lsp::tools::document_info::{LspDocumentInput, LspDocumentOutput, execute_lsp_document};
use crate::lsp::tools::rename::{LspRenameInput, LspRenameOutput, execute_lsp_rename};
use crate::lsp::tools::symbol_lookup::{LspSymbolInput, LspSymbolOutput, execute_lsp_symbol};
use crate::{coding::prompt_rule_matcher::PromptRuleMatcher, lsp::registry::LspRegistry};

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename, truncate};
use tools::bash::{
    BackgroundProcessHandle, BashInput, BashOutput, BashResult, ReadBackgroundBashInput, ReadBackgroundBashOutput,
    execute_command, read_background_bash,
};
use tools::edit_file::{EditFileArgs, EditFileResponse, edit_file_contents};
use tools::find::{FindInput, FindOutput, find_files_by_name};
use tools::grep::{GrepInput, GrepOutput, perform_grep};
use tools::list_files::{ListFilesArgs, ListFilesResult, list_files};
use tools::read_file::{ReadFileArgs, ReadFileResult, read_file_contents};
use tools::web_fetch::{WebFetchInput, WebFetchOutput, WebFetcher};
use tools::web_search::search_client::BraveSearchClient;
use tools::web_search::{WebSearchInput, WebSearchOutput, WebSearcher};
use tools::write_file::{WriteFileArgs, WriteFileResponse, write_file_contents};

/// Extension trait for converting tool results to MCP format
trait IntoMcpResult<T> {
    fn into_mcp(self) -> Result<Json<T>, String>;
}

impl<T, E: std::fmt::Display> IntoMcpResult<T> for Result<T, E> {
    fn into_mcp(self) -> Result<Json<T>, String> {
        self.map(Json).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadSnapshot {
    content_hash: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMutation {
    Edit,
    OverwriteExisting,
}

impl FileMutation {
    fn unread_error(self, file_path: &str) -> String {
        match self {
            Self::Edit => format!(
                "Safety check failed: You must use read_file on '{file_path}' before editing it. This ensures you understand the current file contents before making changes."
            ),
            Self::OverwriteExisting => format!(
                "Safety check failed: File '{file_path}' already exists. You must use read_file on it before overwriting. This prevents accidental data loss."
            ),
        }
    }
}

#[doc = include_str!("../docs/permission_mode.md")]
#[derive(Debug, Clone, Default, PartialEq, clap::ValueEnum)]
pub enum PermissionMode {
    /// Everything auto-executes (current default behavior).
    #[default]
    AlwaysAllow,
    /// File writes auto-execute; destructive bash commands trigger elicitation.
    Auto,
    /// All write/edit/bash calls trigger elicitation; read-only tools are ungated.
    AlwaysAsk,
}

/// CLI arguments for `CodingMcp` server
#[derive(Debug, Clone, Default, Parser)]
pub struct CodingMcpArgs {
    /// Root directory for workspace (used for LSP initialization)
    #[arg(long = "root-dir")]
    pub root_dir: Option<PathBuf>,

    /// Prompt directories to scan for automatic read-triggered rules.
    /// Can be specified multiple times: --rules-dir .aether/skills --rules-dir .claude/rules
    #[arg(long = "rules-dir")]
    pub rules_dirs: Vec<PathBuf>,

    /// Permission mode controlling user approval for tool calls
    #[arg(long = "permission-mode", default_value = "always-allow")]
    pub permission_mode: PermissionMode,

    /// Disable LSP-backed coding tools and daemon connections.
    #[arg(long = "disable-lsp")]
    pub disable_lsp: bool,
}

impl CodingMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        // Prepend a dummy program name since clap expects it
        let mut full_args = vec!["coding-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args).map_err(|e| format!("Failed to parse CodingMcp arguments: {e}"))
    }

    /// Parse the root directory from an mcp.json config file.
    ///
    /// Looks for the "coding" server entry and parses its args for `--root-dir`.
    /// Relative paths (like ".") are resolved against the mcp.json's directory.
    pub fn parse_root_dir_from_config(mcp_config_path: &Path) -> Option<PathBuf> {
        let raw_config = McpConfig::from_json_file(mcp_config_path).ok()?;
        let McpServerConfig::InMemory(InMemoryServerConfig { args, .. }) = raw_config.servers.get("coding")? else {
            return None;
        };

        let parsed_args = Self::from_args(args.clone()).ok()?;
        let root_dir = parsed_args.root_dir?;

        if root_dir.is_relative() {
            let config_dir = mcp_config_path.parent()?;
            Some(config_dir.join(&root_dir).canonicalize().ok()?)
        } else {
            Some(root_dir)
        }
    }
}

#[doc = include_str!("../docs/coding_mcp.md")]
#[derive(Debug)]
pub struct CodingMcp<T: CodingTools = DefaultCodingTools> {
    tool_router: ToolRouter<Self>,
    background_processes: Mutex<HashMap<String, BackgroundProcessHandle>>,
    /// Track read file snapshots to enforce read-before-edit safety
    files_read: RwLock<HashMap<String, ReadSnapshot>>,
    tools: T,
    /// Optional LSP operations (enabled with `.with_lsp()`)
    lsp: Option<Arc<LspRegistry>>,
    web_fetcher: WebFetcher,
    web_searcher: Option<WebSearcher<BraveSearchClient>>,
    /// Workspace roots (from MCP protocol or CLI args)
    roots: RwLock<Vec<PathBuf>>,
    /// Read rules discovered from skill files (activated on file reads)
    read_rule_state: prompt_rule_matcher::PromptRuleMatcher,
    /// Configured prompt directories used to build read rules.
    configured_rules_dirs: Vec<PathBuf>,
    /// Permission mode controlling user approval for tool calls
    permission_mode: PermissionMode,
}

fn build_rule_catalog(configured_rules_dirs: &[PathBuf]) -> aether_project::PromptCatalog {
    if configured_rules_dirs.is_empty() {
        return aether_project::PromptCatalog::empty();
    }

    PromptCatalog::from_dirs(configured_rules_dirs)
}

#[tool_handler(router = self.tool_router)]
impl<T: CodingTools + 'static> ServerHandler for CodingMcp<T> {
    fn get_info(&self) -> ServerInfo {
        let instructions = self.build_instructions();
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("coding-mcp", "0.1.0"))
            .with_instructions(instructions)
    }
}

impl CodingMcp<DefaultCodingTools> {
    /// Create a new `CodingMcp` with default (local filesystem) tools
    pub fn new() -> Self {
        Self::with_tools(DefaultCodingTools::new())
    }
}

async fn notify_preview(context: &RequestContext<RoleServer>, meta: ToolDisplayMeta) {
    if let Some(token) = context.meta.get_progress_token() {
        let result_meta = ToolResultMeta::from(meta);
        let message = serde_json::to_string(&result_meta).unwrap_or_default();
        let _ = context
            .peer
            .notify_progress(ProgressNotificationParam {
                progress_token: token,
                progress: 0.0,
                total: None,
                message: Some(message),
            })
            .await;
    }
}

/// Returns `true` if the command looks destructive (deletes files, force-pushes, etc.).
///
/// Uses simple substring matching — conservative by design.
fn is_dangerous_cmd(command: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "rm ",
        "rm\t",
        "rmdir ",
        "git push",
        "git reset",
        "git checkout --",
        "git clean",
        "chmod ",
        "chown ",
        "kill ",
        "pkill ",
        "mv ",
        "dd ",
        "--force",
        "--hard",
    ];

    // Check simple substring patterns
    if PATTERNS.iter().any(|p| command.contains(p)) {
        return true;
    }

    // Check redirect operators: only match >/>> that aren't inside quotes or part of =>
    // Simple heuristic: look for "> " or ">> " not preceded by '='
    for (i, _) in command.match_indices("> ") {
        if i == 0 || command.as_bytes()[i - 1] != b'=' {
            return true;
        }
    }

    false
}

async fn read_snapshot(file_path: &str) -> Result<(String, ReadSnapshot), String> {
    let path_key = canonical_path_key(file_path).await;
    let content = read_to_string(file_path).await.map_err(|e| format!("Failed to read {file_path}: {e}"))?;
    Ok((path_key, ReadSnapshot { content_hash: content_hash(&content) }))
}

async fn canonical_path_key(file_path: &str) -> String {
    canonicalize(file_path).await.unwrap_or_else(|_| PathBuf::from(file_path)).to_string_lossy().to_string()
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[tool_router]
impl<T: CodingTools + 'static> CodingMcp<T> {
    /// Create a `CodingMcp` with custom tool implementation
    pub fn with_tools(tools: T) -> Self {
        Self {
            tool_router: Self::tool_router(),
            background_processes: Mutex::new(HashMap::new()),
            files_read: RwLock::new(HashMap::new()),
            tools,
            lsp: None,
            web_fetcher: WebFetcher::new(),
            web_searcher: WebSearcher::try_new().ok(),
            roots: RwLock::new(Vec::new()),
            read_rule_state: prompt_rule_matcher::PromptRuleMatcher::default(),
            configured_rules_dirs: Vec::new(),
            permission_mode: PermissionMode::AlwaysAllow,
        }
    }

    /// Enable LSP code intelligence for the given project root.
    ///
    /// LSP servers for detected project languages are spawned immediately
    /// in the background, allowing indexing to start right away.
    pub fn with_lsp(mut self, root_path: PathBuf) -> Self {
        self.lsp = Some(LspRegistry::new_and_spawn(root_path));
        self
    }

    /// Set workspace roots.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        let catalog = build_rule_catalog(&self.configured_rules_dirs);
        self.read_rule_state = prompt_rule_matcher::PromptRuleMatcher::new(catalog);
        self.roots = RwLock::new(roots);
        self
    }

    /// Set prompt directories used for read-triggered rule activation.
    pub fn with_rules_dirs(mut self, rules_dirs: Vec<PathBuf>) -> Self {
        self.configured_rules_dirs = rules_dirs;
        let catalog = build_rule_catalog(&self.configured_rules_dirs);
        self.read_rule_state = PromptRuleMatcher::new(catalog);
        self
    }

    /// Set the workspace root directory from a single path.
    pub fn with_root_dir(self, root_dir: PathBuf) -> Self {
        self.with_roots(vec![root_dir])
    }

    /// Set the permission mode controlling user approval for tool calls.
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Get the current workspace root.
    fn get_workspace_root(&self) -> Option<PathBuf> {
        self.roots.try_read().ok().and_then(|roots| roots.first().cloned())
    }

    fn build_instructions(&self) -> String {
        let mut base = String::from(
            r"# Coding MCP Server

File I/O, search, shell, and optional LSP code intelligence tools for coding workflows.

## Quick Reference

- **Text patterns** (TODOs, logs, strings): `grep`
- **File names** (find *.test.ts): `find`
- **Read/write/edit** files: `read_file`, `write_file`, `edit_file`
- **Shell commands**: `bash`
",
        );

        if self.lsp.is_some() {
            base.push_str(
                r"- **Errors & warnings** (instant check without build): `lsp_check_errors`
- **Code symbols** (definitions, usages, types): `lsp_symbol`
- **File structure** (what's in this file?): `lsp_document`
- **Rename symbol** (refactor across codebase): `lsp_rename`
",
            );
        }

        match self.get_workspace_root() {
            Some(root) => format!(
                r"{}

When using tools that take file paths, always use absolute paths from:
<workspace-root>{}</workspace-root>",
                base,
                root.display()
            ),
            None => base,
        }
    }

    /// Prompt the user for approval via elicitation. Always sends the prompt —
    /// callers are responsible for deciding when to call this based on `permission_mode`.
    ///
    /// Returns `Ok(())` if allowed, or `Err` with either a generic decline
    /// message or the user's feedback explaining what to do instead.
    async fn elicit_permission(
        &self,
        context: &RequestContext<RoleServer>,
        tool_name: &str,
        description: &str,
    ) -> Result<(), String> {
        let message = format!("Allow {tool_name}: {description}?");
        let result = context
            .peer
            .create_elicitation(CreateElicitationRequestParams::FormElicitationParams {
                meta: None,
                message,
                requested_schema: ElicitationSchema::builder()
                    .required_enum_schema(
                        "decision",
                        EnumSchema::builder(vec!["allow".into(), "deny".into()])
                            .untitled()
                            .with_default("deny")
                            .unwrap()
                            .build(),
                    )
                    .build()
                    .unwrap(),
            })
            .await
            .map_err(|e| format!("Elicitation failed: {e}"))?;

        let allowed = result.content.as_ref().and_then(|c| c.get("decision")).and_then(|v| v.as_str()) == Some("allow");

        if allowed { Ok(()) } else { Err(format!("Operation declined by user: {tool_name}")) }
    }

    /// Ask the user for permission to run a bash command. In `Auto` mode only
    /// triggers for destructive commands; in `AlwaysAsk` mode triggers always.
    async fn check_bash_permission(&self, context: &RequestContext<RoleServer>, command: &str) -> Result<(), String> {
        match self.permission_mode {
            PermissionMode::AlwaysAllow => Ok(()),
            PermissionMode::AlwaysAsk => self.elicit_permission(context, "bash", command).await,
            PermissionMode::Auto => {
                if is_dangerous_cmd(command) {
                    self.elicit_permission(context, "bash", command).await
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Ask the user for permission to write/edit a file. Only triggers in
    /// `AlwaysAsk` mode; `Auto` and `AlwaysAllow` auto-approve file mutations.
    async fn check_write_permission(
        &self,
        context: &RequestContext<RoleServer>,
        tool_name: &str,
        file_path: &str,
    ) -> Result<(), String> {
        if self.permission_mode == PermissionMode::AlwaysAsk {
            self.elicit_permission(context, tool_name, file_path).await
        } else {
            Ok(())
        }
    }

    async fn track_read_snapshot(&self, file_path: &str) -> Result<String, String> {
        let (path_key, snapshot) = read_snapshot(file_path).await?;
        self.files_read.write().await.insert(path_key.clone(), snapshot);
        Ok(path_key)
    }

    async fn ensure_fresh_read(&self, file_path: &str, mutation: FileMutation) -> Result<(), String> {
        let (path_key, current_snapshot) = read_snapshot(file_path).await?;
        let files_read = self.files_read.read().await;
        let read_snapshot = files_read.get(&path_key).ok_or_else(|| mutation.unread_error(file_path))?;

        if read_snapshot != &current_snapshot {
            return Err(format!(
                "Safety check failed: File '{file_path}' changed since it was read. Use read_file again before modifying it."
            ));
        }

        Ok(())
    }

    async fn ensure_existing_file_fresh_read(&self, file_path: &str, mutation: FileMutation) -> Result<(), String> {
        if try_exists(file_path).await.map_err(|e| format!("Failed to check existence of {file_path}: {e}"))? {
            self.ensure_fresh_read(file_path, mutation).await?;
        }

        Ok(())
    }

    async fn forget_read_snapshot(&self, file_path: &str) {
        let path_key = canonical_path_key(file_path).await;
        self.files_read.write().await.remove(&path_key);
    }

    fn spawn_diagnostic_refresh(&self, file_path: &str) {
        if let Some(lsp) = &self.lsp {
            let lsp = Arc::clone(lsp);
            let file_path = file_path.to_string();
            tokio::spawn(async move {
                lsp.queue_diagnostic_refresh(&file_path).await;
            });
        }
    }

    #[doc = include_str!("tools/grep/description.md")]
    #[tool]
    pub async fn grep(
        &self,
        request: Parameters<GrepInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GrepOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Grep", format!("'{}'", args.pattern))).await;
        self.tools.grep(args).await.into_mcp()
    }

    #[doc = include_str!("tools/find/description.md")]
    #[tool]
    pub async fn find(
        &self,
        request: Parameters<FindInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<FindOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Find", format!("'{}'", args.pattern))).await;
        self.tools.find(args).await.into_mcp()
    }

    #[doc = include_str!("tools/read_file/description.md")]
    #[tool]
    pub async fn read_file(
        &self,
        request: Parameters<ReadFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ReadFileResult>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Read file", basename(&args.file_path))).await;
        let file_path = args.file_path.clone();
        let mut result = self.tools.read_file(args).await.map_err(|e| e.to_string())?;
        self.track_read_snapshot(&file_path).await?;

        let total_lines = result.total_lines;
        let roots = self.roots.read().await;
        let matched = self.read_rule_state.get_matched_rules(&roots, &file_path);
        for rule in &matched {
            write!(result.content, "\n\n<system-reminder>\n{}\n</system-reminder>", rule.body).unwrap();
        }

        if !matched.is_empty() {
            let rule_names: Vec<&str> = matched.iter().map(|r| r.name.as_str()).collect();
            let base = format!("{}, {total_lines} lines", basename(&file_path));
            let value = format!("{base} +rules: {}", rule_names.join(", "));
            result.meta = Some(ToolDisplayMeta::new("Read file", value).into());
        }

        Ok(Json(result))
    }

    #[doc = include_str!("tools/write_file/description.md")]
    #[tool]
    pub async fn write_file(
        &self,
        request: Parameters<WriteFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<WriteFileResponse>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Write file", basename(&args.file_path))).await;

        self.check_write_permission(&context, "write_file", &args.file_path).await?;

        self.ensure_existing_file_fresh_read(&args.file_path, FileMutation::OverwriteExisting).await?;

        let response = self.tools.write_file(args).await.map_err(|e| e.to_string())?;

        self.forget_read_snapshot(&response.file_path).await;

        self.spawn_diagnostic_refresh(&response.file_path);

        Ok(Json(response))
    }

    #[doc = include_str!("tools/edit_file/description.md")]
    #[tool]
    pub async fn edit_file(
        &self,
        request: Parameters<EditFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<EditFileResponse>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Edit file", basename(&args.file_path))).await;

        self.check_write_permission(&context, "edit_file", &args.file_path).await?;

        self.ensure_fresh_read(&args.file_path, FileMutation::Edit).await?;

        let response = self.tools.edit_file(args).await.map_err(|e| e.to_string())?;

        self.forget_read_snapshot(&response.file_path).await;

        self.spawn_diagnostic_refresh(&response.file_path);

        Ok(Json(response))
    }

    #[doc = include_str!("tools/list_files/description.md")]
    #[tool]
    pub async fn list_files(
        &self,
        request: Parameters<ListFilesArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ListFilesResult>, String> {
        let Parameters(args) = request;
        let preview_value = args.path.as_deref().map_or_else(|| ".".to_string(), basename);
        notify_preview(&context, ToolDisplayMeta::new("List files", preview_value)).await;
        self.tools.list_files(args).await.into_mcp()
    }

    #[doc = include_str!("tools/bash/description.md")]
    #[tool]
    pub async fn bash(
        &self,
        request: Parameters<BashInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<BashOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Run command", truncate(&args.command, 40))).await;

        self.check_bash_permission(&context, &args.command).await?;

        let command = args.command.clone();
        let result = self.tools.bash(args).await.map_err(|e| e.to_string())?;

        match result {
            BashResult::Completed(output) => Ok(Json(output)),
            BashResult::Background(handle) => {
                let shell_id = handle.shell_id.clone();

                // Store the background process
                self.background_processes.lock().await.insert(shell_id.clone(), handle);

                let display_meta =
                    ToolDisplayMeta::new("Run command", format!("{} (background)", truncate(&command, 40)));

                // Return immediate response with shell_id
                Ok(Json(BashOutput {
                    output: String::new(),
                    exit_code: 0,
                    killed: None,
                    shell_id: Some(shell_id),
                    meta: Some(display_meta.into()),
                }))
            }
        }
    }

    #[doc = include_str!("tools/bash/read_background_description.md")]
    #[tool]
    pub async fn read_background_bash(
        &self,
        request: Parameters<ReadBackgroundBashInput>,
    ) -> Result<Json<ReadBackgroundBashOutput>, String> {
        let Parameters(args) = request;

        let handle = self
            .background_processes
            .lock()
            .await
            .remove(&args.bash_id)
            .ok_or_else(|| format!("Shell ID not found: {}", args.bash_id))?;

        let (result, handle_opt) =
            self.tools.read_background_bash(handle, args.filter).await.map_err(|e| e.to_string())?;

        // Put handle back if still running
        if let Some(handle) = handle_opt {
            self.background_processes.lock().await.insert(args.bash_id, handle);
        }

        Ok(Json(result))
    }

    #[doc = include_str!("tools/web_fetch/description.md")]
    #[tool]
    pub async fn web_fetch(
        &self,
        request: Parameters<WebFetchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<WebFetchOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Fetch URL", truncate(&args.url, 60))).await;
        self.web_fetcher.fetch(args).await.into_mcp()
    }

    #[doc = include_str!("tools/web_search/description.md")]
    #[tool]
    pub async fn web_search(
        &self,
        request: Parameters<WebSearchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<WebSearchOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Web search", format!("'{}'", args.query))).await;

        let searcher = self.web_searcher.as_ref().ok_or_else(|| {
            "Web search not available: BRAVE_SEARCH_API_KEY environment variable not set. \
                 Get a free API key from https://api.search.brave.com/app/keys"
                .to_string()
        })?;

        searcher.search(args).await.map_err(|e| e.to_string()).map(Json)
    }

    #[doc = include_str!("../lsp/tools/symbol_lookup/description.md")]
    #[tool]
    pub async fn lsp_symbol(
        &self,
        request: Parameters<LspSymbolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspSymbolOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP symbol", &input.symbol)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_symbol(input, lsp.as_ref()).await.map(Json)
    }

    #[doc = include_str!("../lsp/tools/document_info/description.md")]
    #[tool]
    pub async fn lsp_document(
        &self,
        request: Parameters<LspDocumentInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspDocumentOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP document", basename(&input.file_path))).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_document(input, lsp.as_ref()).await.map(Json)
    }

    #[doc = include_str!("../lsp/tools/check_errors/description.md")]
    #[tool]
    pub async fn lsp_check_errors(
        &self,
        request: Parameters<LspDiagnosticsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspDiagnosticsOutput>, String> {
        let Parameters(request) = request;
        let preview_value = match &request.input {
            LspDiagnosticsInput::Workspace {} => "workspace".to_string(),
            LspDiagnosticsInput::File { file_path } => basename(file_path),
        };
        notify_preview(&context, ToolDisplayMeta::new("LSP errors", preview_value)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_diagnostics(request, lsp.as_ref()).await.map(Json)
    }

    #[doc = include_str!("../lsp/tools/rename/description.md")]
    #[tool]
    pub async fn lsp_rename(
        &self,
        request: Parameters<LspRenameInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspRenameOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP rename", &input.symbol)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_rename(input, lsp.as_ref()).await.map(Json)
    }
}

impl Default for CodingMcp<DefaultCodingTools> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "test-helpers")]
impl<T: CodingTools + 'static> CodingMcp<T> {
    /// Read a file and track it in the read set (test helper, no MCP context needed).
    pub async fn test_read_file(&self, args: ReadFileArgs) -> Result<Json<ReadFileResult>, String> {
        let file_path = args.file_path.clone();
        let mut result = self.tools.read_file(args).await.map_err(|e| e.to_string())?;
        self.track_read_snapshot(&file_path).await?;

        let total_lines = result.total_lines;
        let roots = self.roots.read().await;
        let matched = self.read_rule_state.get_matched_rules(&roots, &file_path);
        for rule in &matched {
            write!(result.content, "\n\n<system-reminder>\n{}\n</system-reminder>", rule.body).unwrap();
        }

        if !matched.is_empty() {
            let rule_names: Vec<&str> = matched.iter().map(|r| r.name.as_str()).collect();
            let base = format!("{}, {total_lines} lines", basename(&file_path));
            let value = format!("{base} +rules: {}", rule_names.join(", "));
            result.meta = Some(ToolDisplayMeta::new("Read file", value).into());
        }

        Ok(Json(result))
    }

    /// Write a file with read-before-write safety check (test helper, no MCP context needed).
    pub async fn test_write_file(&self, args: WriteFileArgs) -> Result<Json<WriteFileResponse>, String> {
        self.ensure_existing_file_fresh_read(&args.file_path, FileMutation::OverwriteExisting).await?;
        let response = self.tools.write_file(args).await.map_err(|e| e.to_string())?;
        self.forget_read_snapshot(&response.file_path).await;
        Ok(Json(response))
    }

    /// Edit a file with read-before-edit safety check (test helper, no MCP context needed).
    pub async fn test_edit_file(&self, args: EditFileArgs) -> Result<Json<EditFileResponse>, String> {
        self.ensure_fresh_read(&args.file_path, FileMutation::Edit).await?;
        let response = self.tools.edit_file(args).await.map_err(|e| e.to_string())?;
        self.forget_read_snapshot(&response.file_path).await;
        Ok(Json(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_default_permission_mode_is_always_allow() {
        let args = CodingMcpArgs::from_args(vec![]).unwrap();
        assert_eq!(args.permission_mode, PermissionMode::AlwaysAllow);
    }

    #[test]
    fn args_default_lsp_enabled() {
        let args = CodingMcpArgs::from_args(vec![]).unwrap();
        assert!(!args.disable_lsp);
    }

    #[test]
    fn args_parses_disable_lsp() {
        let args = CodingMcpArgs::from_args(vec!["--disable-lsp".into()]).unwrap();
        assert!(args.disable_lsp);
    }

    #[test]
    fn disabled_lsp_instructions_omit_lsp_tools() {
        let instructions = CodingMcp::new().build_instructions();
        assert!(!instructions.contains("lsp_check_errors"));
        assert!(!instructions.contains("lsp_symbol"));
    }

    #[test]
    fn args_parses_always_allow() {
        let args = CodingMcpArgs::from_args(vec!["--permission-mode".into(), "always-allow".into()]).unwrap();
        assert_eq!(args.permission_mode, PermissionMode::AlwaysAllow);
    }

    #[test]
    fn args_parses_auto() {
        let args = CodingMcpArgs::from_args(vec!["--permission-mode".into(), "auto".into()]).unwrap();
        assert_eq!(args.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn args_parses_always_ask() {
        let args = CodingMcpArgs::from_args(vec!["--permission-mode".into(), "always-ask".into()]).unwrap();
        assert_eq!(args.permission_mode, PermissionMode::AlwaysAsk);
    }

    #[test]
    fn args_rejects_invalid_permission_mode() {
        assert!(CodingMcpArgs::from_args(vec!["--permission-mode".into(), "yolo".into()]).is_err());
    }

    #[test]
    fn args_parses_repeated_rules_dirs() {
        let args = CodingMcpArgs::from_args(vec![
            "--rules-dir".into(),
            ".aether/skills".into(),
            "--rules-dir".into(),
            ".claude/rules".into(),
        ])
        .unwrap();

        assert_eq!(args.rules_dirs, vec![PathBuf::from(".aether/skills"), PathBuf::from(".claude/rules")]);
    }

    #[test]
    fn with_permission_mode_stores_mode() {
        let mcp = CodingMcp::new().with_permission_mode(PermissionMode::AlwaysAsk);
        assert_eq!(mcp.permission_mode, PermissionMode::AlwaysAsk);
    }

    #[test]
    fn default_permission_mode_is_always_allow() {
        let mcp = CodingMcp::new();
        assert_eq!(mcp.permission_mode, PermissionMode::AlwaysAllow);
    }

    #[test]
    fn detects_rm() {
        assert!(is_dangerous_cmd("rm -rf /tmp/foo"));
        assert!(is_dangerous_cmd("rm\tfoo.txt"));
    }

    #[test]
    fn detects_git_push() {
        assert!(is_dangerous_cmd("git push origin main"));
    }

    #[test]
    fn detects_git_reset() {
        assert!(is_dangerous_cmd("git reset --hard HEAD~1"));
    }

    #[test]
    fn detects_git_checkout_discard() {
        assert!(is_dangerous_cmd("git checkout -- ."));
    }

    #[test]
    fn detects_git_clean() {
        assert!(is_dangerous_cmd("git clean -fd"));
    }

    #[test]
    fn detects_redirect() {
        assert!(is_dangerous_cmd("echo x > file.txt"));
        assert!(is_dangerous_cmd("echo x >> file.txt"));
    }

    #[test]
    fn does_not_flag_fat_arrow() {
        assert!(!is_dangerous_cmd("grep '=> ' file.txt"));
    }

    #[test]
    fn detects_chmod_chown() {
        assert!(is_dangerous_cmd("chmod 777 /etc/passwd"));
        assert!(is_dangerous_cmd("chown root:root /tmp/x"));
    }

    #[test]
    fn detects_kill_signals() {
        assert!(is_dangerous_cmd("kill -9 1234"));
        assert!(is_dangerous_cmd("pkill node"));
    }

    #[test]
    fn does_not_flag_kill_substring() {
        assert!(!is_dangerous_cmd("echo skillset"));
    }

    #[test]
    fn detects_mv() {
        assert!(is_dangerous_cmd("mv old.txt new.txt"));
    }

    #[test]
    fn detects_force_flags() {
        assert!(is_dangerous_cmd("npm install --force"));
        assert!(is_dangerous_cmd("git reset --hard"));
    }

    #[test]
    fn detects_dd() {
        assert!(is_dangerous_cmd("dd if=/dev/zero of=/dev/sda"));
    }

    #[test]
    fn detects_rmdir() {
        assert!(is_dangerous_cmd("rmdir empty_dir"));
    }

    #[test]
    fn does_not_flag_redirect_in_output() {
        assert!(is_dangerous_cmd("> file.txt"));
    }

    #[test]
    fn allows_safe_commands() {
        assert!(!is_dangerous_cmd("ls -la"));
        assert!(!is_dangerous_cmd("cat foo.txt"));
        assert!(!is_dangerous_cmd("git status"));
        assert!(!is_dangerous_cmd("git diff"));
        assert!(!is_dangerous_cmd("cargo test"));
        assert!(!is_dangerous_cmd("grep -r pattern ."));
    }
}
