use aether_project::PromptCatalog;
use clap::Parser;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::ToolCallContext,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities, ContentBlock,
        CreateTaskResult, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema,
        EnumSchema, GetTaskParams, GetTaskResult, Implementation, InputRequest, InputRequests,
        ProgressNotificationParam, ServerCapabilities, ServerInfo, UpdateTaskParams,
    },
    service::RequestContext,
    task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions},
    tool, tool_handler, tool_router,
};
use std::fmt::{Debug, Formatter, Write as _};
use std::path::PathBuf;
use std::{collections::HashSet, sync::Arc};
use tokio::{fs::try_exists, sync::RwLock};

pub mod default_tools;
pub mod error;
pub mod prompt_rule_matcher;
pub mod tools;
pub mod tools_trait;

pub use default_tools::DefaultCodingTools;
pub use tools_trait::CodingTools;

use crate::lsp::tools::check_errors::{LspDiagnosticsOutput, LspDiagnosticsRequest, execute_lsp_diagnostics};
use crate::lsp::tools::symbol_lookup::{LspSymbolInput, LspSymbolOutput, execute_lsp_symbol};
use crate::lsp::tools::workspace_search::{
    LspWorkspaceSearchInput, LspWorkspaceSearchOutput, execute_lsp_workspace_search,
};
use crate::{coding::prompt_rule_matcher::PromptRuleMatcher, lsp::registry::LspRegistry};
use crate::{
    error::ServerInitError,
    lsp::tools::rename::{LspRenameInput, LspRenameOutput, execute_lsp_rename},
};
use crate::{
    lsp::tools::document_info::{LspDocumentInput, LspDocumentOutput, execute_lsp_document},
    workspace_paths::WorkspacePaths,
};
use mcp_utils::server::mrtr::{MrtrAction, get_next_mrtr_action, parse_response};
use mcp_utils::server::tool::parse_arguments;

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename, truncate};
use tools::ast_grep::{AstGrepInput, AstGrepOutput, perform_ast_grep};
use tools::bash::{BashInput, BashOutput, validate_args};
use tools::edit_file::{EditFileArgs, EditFileResponse, edit_file_contents};
use tools::find::{FindInput, FindOutput, find_files};
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

#[doc = include_str!("../docs/permission_mode.md")]
#[derive(Debug, Clone, Default, PartialEq, clap::ValueEnum)]
pub enum PermissionMode {
    /// Everything auto-executes (current default behavior).
    #[default]
    AlwaysAllow,
    /// File writes auto-execute; destructive bash commands trigger elicitation.
    Auto,
    /// Every destructive-annotated tool call triggers elicitation; read-only
    /// tools are ungated.
    AlwaysAsk,
}

/// CLI arguments for `CodingMcp` server
#[derive(Debug, Clone, Default, Parser)]
pub struct CodingMcpArgs {
    /// Root directory for path resolution and LSP initialization.
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
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["coding-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }
}

#[doc = include_str!("../docs/coding_mcp.md")]
pub struct CodingMcp<T: CodingTools = DefaultCodingTools> {
    tool_router: ToolRouter<Self>,
    task_manager: TaskManager,
    /// Track files that have been read to enforce read-before-edit safety
    files_read: RwLock<HashSet<String>>,
    tools: Arc<T>,
    /// Optional LSP operations (enabled with `.with_lsp()`)
    lsp: Option<Arc<LspRegistry>>,
    web_fetcher: WebFetcher,
    web_searcher: Option<WebSearcher<BraveSearchClient>>,
    /// Root directory used for path resolution and tool instructions.
    root_dir: PathBuf,
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_tasks().build())
            .with_server_info(Implementation::new("coding-mcp", "0.1.0"))
            .with_instructions(instructions)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let background_args = background_bash_args(&request);
        if let Some(args) = &background_args {
            if !context.client_capabilities().is_some_and(|capabilities| capabilities.supports_tasks()) {
                return Err(ErrorData::missing_required_client_capability(
                    ClientCapabilities::builder().enable_tasks().build(),
                ));
            }
            validate_args(args).map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
        }

        if let Some(prompt) = self.permission_prompt(&request) {
            let action = get_next_mrtr_action(request.input_responses.as_ref(), || {
                Ok(InputRequests::from([(
                    "permission".to_string(),
                    InputRequest::Elicitation(ElicitRequest::new(decision_form(
                        &prompt.tool_name,
                        &prompt.description,
                    ))),
                )]))
            })?
            .validate_for_client(&context);

            match action {
                MrtrAction::Request(request) => return Ok(request.into()),
                MrtrAction::Abort(_) => {
                    let message = permission_unsupported(&prompt.tool_name);
                    return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
                }
                MrtrAction::Resume(responses) => {
                    let result: ElicitResult = parse_response(&responses, "permission")?;
                    if !permission_granted(&result) {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "Operation declined by user: {}",
                            prompt.tool_name
                        ))])
                        .into());
                    }
                }
            }
        }
        if let Some(args) = background_args {
            return Ok(CallToolResponse::Task(CreateTaskResult::new(self.create_background_bash_task(args))));
        }

        self.tool_router.call(ToolCallContext::new(self, request, context)).await
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        Ok(GetTaskResult::new(self.task_manager.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.task_manager.update_task(&request.task_id, request.input_responses)?;
        Ok(())
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.task_manager.cancel_task(&request.task_id)?;
        Ok(())
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
        let _ = context.peer.notify_progress(ProgressNotificationParam::new(token, 0.0).with_message(message)).await;
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

/// Bounds both how long a background command may run and how long its finished
/// result stays pollable — rmcp couples the hard-stop and terminal-entry
/// eviction to this single TTL. `None` would retain every finished task's
/// output for the server's lifetime.
const BACKGROUND_TASK_TTL_MS: u64 = 3_600_000;

#[tool_router]
impl<T: CodingTools + 'static> CodingMcp<T> {
    /// Create a `CodingMcp` with custom tool implementation
    pub fn with_tools(tools: T) -> Self {
        Self {
            tool_router: Self::tool_router(),
            task_manager: TaskManager::new(),
            files_read: RwLock::new(HashSet::new()),
            tools: Arc::new(tools),
            lsp: None,
            web_fetcher: WebFetcher::new(),
            web_searcher: WebSearcher::try_new().ok(),
            root_dir: crate::workspace_paths::current_dir(),
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

    /// Set the root directory.
    pub fn with_root_dir(mut self, root_dir: PathBuf) -> Self {
        self.root_dir = root_dir;
        self
    }

    /// Set prompt directories used for read-triggered rule activation.
    pub fn with_rules_dirs(mut self, rules_dirs: Vec<PathBuf>) -> Self {
        self.configured_rules_dirs = rules_dirs;
        let catalog = build_rule_catalog(&self.configured_rules_dirs);
        self.read_rule_state = PromptRuleMatcher::new(catalog);
        self
    }

    /// Set the permission mode controlling user approval for tool calls.
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    fn create_background_bash_task(&self, args: BashInput) -> rmcp::model::Task {
        let tools = Arc::clone(&self.tools);
        let cwd = self.root_dir.clone();
        let description = args.description.clone().unwrap_or_else(|| truncate(&args.command, 80));
        let options = TaskOptions::new().with_ttl_ms(BACKGROUND_TASK_TTL_MS).with_status_message(description);

        self.task_manager.spawn(options, move |task_context: TaskContext| {
            Box::pin(async move {
                tokio::select! {
                    () = task_context.cancelled() => Err(TaskExit::Cancelled),
                    result = tools.bash(args, Some(cwd)) => Ok(match result {
                        Ok(output) => serde_json::to_value(output).map_or_else(
                            |error| CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
                            CallToolResult::structured,
                        ),
                        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
                    }),
                }
            })
        })
    }

    fn workspace_paths(&self) -> WorkspacePaths {
        WorkspacePaths::new(self.root_dir.clone())
    }

    fn build_instructions(&self) -> String {
        let mut base = String::from(
            r"# Coding MCP Server

File I/O, search, shell, and optional LSP code intelligence tools for coding workflows.

## Quick Reference

- **Text patterns** (TODOs, logs, strings): `grep`
- **Structural code patterns** (AST search): `ast_grep`
- **File names** (find *.test.ts): `find`
- **Read/write/edit** files: `read_file`, `write_file`, `edit_file`
- **Shell commands**: `bash`
",
        );

        if self.lsp.is_some() {
            base.push_str(
                r"- **Errors & warnings** (instant check without build): `lsp_check_errors`
- **Code symbols** (definitions, usages, types): `lsp_symbol`
- **Find symbol across workspace** (don't know the file?): `lsp_workspace_search`
- **File structure** (what's in this file?): `lsp_document`
- **Rename symbol** (refactor across codebase): `lsp_rename`
",
            );
        }

        format!(
            r"{}

When using tools that take file paths, always use absolute paths from:
<workspace-root>{}</workspace-root>",
            base,
            self.root_dir.display()
        )
    }

    /// Decide whether a tool call needs user approval before it may run.
    /// Gating is derived from the tool's `destructive_hint` annotation:
    /// `AlwaysAsk` gates every destructive tool, `Auto` gates only bash
    /// commands the classifier deems dangerous. Malformed arguments pass
    /// through ungated so the tool itself reports the parse error.
    fn permission_prompt(&self, request: &CallToolRequestParams) -> Option<PermissionPrompt> {
        let name = request.name.as_ref();
        let destructive = self
            .tool_router
            .get(name)
            .and_then(|tool| tool.annotations.as_ref())
            .and_then(|annotations| annotations.destructive_hint)
            .unwrap_or(false);
        if !destructive {
            return None;
        }

        match self.permission_mode {
            PermissionMode::AlwaysAllow => None,
            PermissionMode::Auto => {
                if name != "bash" {
                    return None;
                }
                let command = parse_arguments::<BashInput>(request).ok()?.command;
                is_dangerous_cmd(&command)
                    .then(|| PermissionPrompt { tool_name: name.to_string(), description: command })
            }
            PermissionMode::AlwaysAsk => {
                let description = match name {
                    "bash" => parse_arguments::<BashInput>(request).ok()?.command,
                    "write_file" | "edit_file" => {
                        let file_path = parse_arguments::<FilePathArg>(request).ok()?.file_path;
                        self.resolve_file_arg(&file_path).unwrap_or(file_path)
                    }
                    _ => {
                        let args = request.arguments.as_ref().and_then(|a| serde_json::to_string(a).ok());
                        truncate(&args.unwrap_or_default(), 80)
                    }
                };
                Some(PermissionPrompt { tool_name: name.to_string(), description })
            }
        }
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

    /// Resolves a required file-path argument against the root directory,
    /// returning the normalized absolute path as a string.
    fn resolve_file_arg(&self, raw: &str) -> Result<String, String> {
        Ok(self.workspace_paths().resolve_file(raw).map_err(|e| e.to_string())?.to_string_lossy().to_string())
    }

    /// Reads `args.file_path` (already resolved against the root directory),
    /// records it in the read set, and appends any matching read-rule reminders.
    async fn read_and_track(&self, args: ReadFileArgs) -> Result<Json<ReadFileResult>, String> {
        let file_path = args.file_path.clone();
        let mut result = self.tools.read_file(args).await.map_err(|e| e.to_string())?;
        self.files_read.write().await.insert(file_path.clone());

        let total_lines = result.total_lines;
        let matched = self.read_rule_state.get_matched_rules(&self.root_dir, &file_path);
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

    /// Read-before-overwrite safety check: an existing file must have been read
    /// first, preventing accidental data loss.
    async fn ensure_read_before_overwrite(&self, file_path: &str) -> Result<(), String> {
        if try_exists(file_path).await.map_err(|e| format!("Failed to check existence of {file_path}: {e}"))?
            && !self.files_read.read().await.contains(file_path)
        {
            return Err(format!(
                "Safety check failed: File '{file_path}' already exists. You must use read_file on it before overwriting. This prevents accidental data loss."
            ));
        }
        Ok(())
    }

    /// Read-before-edit safety check: a file must have been read before editing.
    async fn ensure_read_before_edit(&self, file_path: &str) -> Result<(), String> {
        if !self.files_read.read().await.contains(file_path) {
            return Err(format!(
                "Safety check failed: You must use read_file on '{file_path}' before editing it. This ensures you understand the current file contents before making changes."
            ));
        }
        Ok(())
    }

    #[doc = include_str!("tools/grep/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn grep(
        &self,
        request: Parameters<GrepInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GrepOutput>, String> {
        let Parameters(mut args) = request;
        let normalized_path = self.workspace_paths().resolve_dir(args.path.as_deref());
        args.path = Some(normalized_path.to_string_lossy().to_string());
        notify_preview(&context, ToolDisplayMeta::new("Grep", format!("'{}'", args.pattern))).await;
        self.tools.grep(args).await.into_mcp()
    }

    #[doc = include_str!("tools/ast_grep/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn ast_grep(
        &self,
        request: Parameters<AstGrepInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<AstGrepOutput>, String> {
        let Parameters(mut args) = request;
        let normalized_path = self.workspace_paths().resolve_dir(args.path.as_deref());
        args.path = Some(normalized_path.to_string_lossy().to_string());
        notify_preview(&context, ToolDisplayMeta::new("AST grep", format!("'{}'", args.pattern))).await;
        self.tools.ast_grep(args).await.into_mcp()
    }

    #[doc = include_str!("tools/find/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn find(
        &self,
        request: Parameters<FindInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<FindOutput>, String> {
        let Parameters(mut args) = request;
        let normalized_path = self.workspace_paths().resolve_dir(args.path.as_deref());
        args.path = Some(normalized_path.to_string_lossy().to_string());
        notify_preview(&context, ToolDisplayMeta::new("Find", format!("'{}'", args.pattern))).await;
        self.tools.find(args).await.into_mcp()
    }

    #[doc = include_str!("tools/read_file/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn read_file(
        &self,
        request: Parameters<ReadFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ReadFileResult>, String> {
        let Parameters(mut args) = request;
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Read file", basename(&args.file_path))).await;
        self.read_and_track(args).await
    }

    #[doc = include_str!("tools/write_file/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn write_file(
        &self,
        request: Parameters<WriteFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<WriteFileResponse>, String> {
        let Parameters(mut args) = request;
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Write file", basename(&args.file_path))).await;

        self.ensure_read_before_overwrite(&args.file_path).await?;

        let response = self.tools.write_file(args).await.map_err(|e| e.to_string())?;

        self.spawn_diagnostic_refresh(&response.file_path);

        Ok(Json(response))
    }

    #[doc = include_str!("tools/edit_file/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn edit_file(
        &self,
        request: Parameters<EditFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<EditFileResponse>, String> {
        let Parameters(mut args) = request;
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Edit file", basename(&args.file_path))).await;

        self.ensure_read_before_edit(&args.file_path).await?;

        let response = self.tools.edit_file(args).await.map_err(|e| e.to_string())?;

        self.spawn_diagnostic_refresh(&response.file_path);

        Ok(Json(response))
    }

    #[doc = include_str!("tools/list_files/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn list_files(
        &self,
        request: Parameters<ListFilesArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ListFilesResult>, String> {
        let Parameters(mut args) = request;
        let normalized_path = self.workspace_paths().resolve_dir(args.path.as_deref());
        let preview_value = basename(&normalized_path.to_string_lossy());
        args.path = Some(normalized_path.to_string_lossy().to_string());
        notify_preview(&context, ToolDisplayMeta::new("List files", preview_value)).await;
        self.tools.list_files(args).await.into_mcp()
    }

    #[doc = include_str!("tools/bash/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = true
    ))]
    pub async fn bash(
        &self,
        request: Parameters<BashInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<BashOutput>, String> {
        let Parameters(args) = request;
        notify_preview(&context, ToolDisplayMeta::new("Run command", truncate(&args.command, 40))).await;

        let cwd = self.root_dir.clone();
        let result = self.tools.bash(args, Some(cwd)).await.map_err(|e| e.to_string())?;
        Ok(Json(result))
    }

    #[doc = include_str!("tools/web_fetch/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = true))]
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
    #[tool(annotations(read_only_hint = true, open_world_hint = true))]
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

        let peer = context.peer.clone();
        let progress_token = context.meta.get_progress_token();
        searcher
            .search(args, move |delay| {
                let peer = peer.clone();
                let progress_token = progress_token.clone();
                async move {
                    if let Some(token) = progress_token {
                        let message = format!("Search rate limited; retrying in {} seconds.", delay.as_secs());
                        let _ = peer
                            .notify_progress(ProgressNotificationParam::new(token, 0.0).with_message(message))
                            .await;
                    }
                }
            })
            .await
            .map_err(|e| e.to_string())
            .map(Json)
    }

    #[doc = include_str!("../lsp/tools/symbol_lookup/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn lsp_symbol(
        &self,
        request: Parameters<LspSymbolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspSymbolOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP symbol", &input.symbol)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_symbol(input, lsp.as_ref()).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("../lsp/tools/workspace_search/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn lsp_workspace_search(
        &self,
        request: Parameters<LspWorkspaceSearchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspWorkspaceSearchOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP search", format!("'{}'", input.query))).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_workspace_search(input, lsp.as_ref()).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("../lsp/tools/document_info/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn lsp_document(
        &self,
        request: Parameters<LspDocumentInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspDocumentOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP document", basename(&input.file_path))).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_document(input, lsp.as_ref()).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("../lsp/tools/check_errors/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn lsp_check_errors(
        &self,
        request: Parameters<LspDiagnosticsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspDiagnosticsOutput>, String> {
        let Parameters(request) = request;
        let preview_value = request.file_path.as_ref().map_or_else(|| "workspace".to_string(), |path| basename(path));
        notify_preview(&context, ToolDisplayMeta::new("LSP errors", preview_value)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_diagnostics(request, lsp.as_ref()).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("../lsp/tools/rename/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn lsp_rename(
        &self,
        request: Parameters<LspRenameInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<LspRenameOutput>, String> {
        let Parameters(input) = request;
        notify_preview(&context, ToolDisplayMeta::new("LSP rename", &input.symbol)).await;
        let lsp = self.lsp.as_ref().ok_or("LSP not configured")?;
        execute_lsp_rename(input, lsp.as_ref()).await.map(Json).map_err(|e| e.to_string())
    }
}

/// A pending user-approval prompt for a gated tool call.
struct PermissionPrompt {
    tool_name: String,
    description: String,
}

/// The file-path argument shared by `write_file` and `edit_file`, extracted
/// for permission descriptions without parsing the full tool input. Must stay
/// in sync with the serde naming of `WriteFileArgs`/`EditFileArgs`.
#[derive(serde::Deserialize)]
struct FilePathArg {
    #[serde(rename = "filePath", alias = "file_path")]
    file_path: String,
}

fn permission_granted(result: &ElicitResult) -> bool {
    result.action == ElicitationAction::Accept
        && result.content.as_ref().and_then(|c| c.get("decision")).and_then(|v| v.as_str()) == Some("allow")
}

fn permission_unsupported(tool_name: &str) -> String {
    format!(
        "Permission required for {tool_name}, but the connected client cannot prompt the user \
         (MCP elicitation over protocol 2026-07-28 or newer). Use --permission-mode always-allow \
         or an elicitation-capable client."
    )
}

fn decision_form(tool_name: &str, description: &str) -> ElicitRequestParams {
    ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: format!("Allow {tool_name}: {description}?"),
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
    }
}

fn background_bash_args(request: &CallToolRequestParams) -> Option<BashInput> {
    if request.name.as_ref() != "bash" {
        return None;
    }

    let args = parse_arguments::<BashInput>(request).ok()?;
    args.run_in_background.is_some_and(|background| background).then_some(args)
}

// TaskManager has no Drop of its own: aborting the tasks here drops their
// in-flight command futures, whose kill_on_drop reaps the child processes.
impl<T: CodingTools> Drop for CodingMcp<T> {
    fn drop(&mut self) {
        self.task_manager.shutdown();
    }
}

impl<T: CodingTools> Debug for CodingMcp<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodingMcp")
            .field("root_dir", &self.root_dir)
            .field("permission_mode", &self.permission_mode)
            .finish_non_exhaustive()
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
    pub async fn test_read_file(&self, mut args: ReadFileArgs) -> Result<Json<ReadFileResult>, String> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        self.read_and_track(args).await
    }

    /// Write a file with read-before-write safety check (test helper, no MCP context needed).
    pub async fn test_write_file(&self, mut args: WriteFileArgs) -> Result<Json<WriteFileResponse>, String> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        self.ensure_read_before_overwrite(&args.file_path).await?;
        self.tools.write_file(args).await.map(Json).map_err(|e| e.to_string())
    }

    /// Edit a file with read-before-edit safety check (test helper, no MCP context needed).
    pub async fn test_edit_file(&self, mut args: EditFileArgs) -> Result<Json<EditFileResponse>, String> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        self.ensure_read_before_edit(&args.file_path).await?;
        self.tools.edit_file(args).await.map(Json).map_err(|e| e.to_string())
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
