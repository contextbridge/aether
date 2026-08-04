use aether_project::PromptCatalog;
use clap::Parser;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequestParams, ElicitationAction,
        ElicitationSchema, EnumSchema, ErrorData, Implementation, ProgressNotificationParam, ServerCapabilities,
        ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{
    fs::try_exists,
    sync::{Mutex, RwLock},
};

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

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename, truncate};
use tools::ast_grep::{AstGrepInput, AstGrepOutput, perform_ast_grep};
use tools::bash::{
    BackgroundProcessHandle, BashInput, BashOutput, BashResult, ReadBackgroundBashInput, ReadBackgroundBashOutput,
    execute_command_in_dir, read_background_bash,
};
use tools::edit_file::{EditFileArgs, EditFileResponse, edit_file_contents};
use tools::find::{FindInput, FindOutput, find_files};
use tools::grep::{GrepInput, GrepOutput, perform_grep};
use tools::list_files::{ListFilesArgs, ListFilesResult, list_files};
use tools::read_file::{ReadFileArgs, ReadFileResult, read_file_contents};
use tools::web_fetch::{WebFetchInput, WebFetchOutput, WebFetcher};
use tools::web_search::search_client::BraveSearchClient;
use tools::web_search::{WebSearchInput, WebSearchOutput, WebSearcher};
use tools::write_file::{WriteFileArgs, WriteFileResponse, write_file_contents};

use crate::mrtr::{
    elicitation_result, input_required, new_request_state_codec, require_form_elicitation, route_tool_call,
    seal_request_state, verify_request_state,
};

/// Wire key used for the allow/deny decision in permission elicitation forms
/// and for the corresponding `inputResponses` entry on MRTR retries.
const PERMISSION_INPUT_KEY: &str = "decision";

impl<T: CodingTools + 'static> CodingMcp<T> {
    /// Manual `call_tool` interception for permission-gated tools: rmcp's tool
    /// router drops `input_responses`/`request_state` when it builds
    /// `ToolCallContext`, so approval rounds are resolved here and every other
    /// tool is delegated to the generated router untouched.
    async fn intercept_call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.permission_mode != PermissionMode::AlwaysAllow && Self::is_permission_gated(request.name.as_ref()) {
            if request.request_state.is_some() {
                return self.resolve_permission_round(request, context).await;
            }
            if request.input_responses.is_some() {
                return Err(ErrorData::invalid_params(
                    "permission retry carried inputResponses without requestState",
                    None,
                ));
            }
            if let Some(response) = self.start_permission_round(&request, &context)? {
                return Ok(response);
            }
        }
        route_tool_call(self, &self.tool_router, request, context).await
    }
}

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
    /// All write/edit/bash calls trigger elicitation; read-only tools are ungated.
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
#[derive(Debug)]
pub struct CodingMcp<T: CodingTools = DefaultCodingTools> {
    tool_router: ToolRouter<Self>,
    background_processes: Mutex<HashMap<String, BackgroundProcessHandle>>,
    /// Track files that have been read to enforce read-before-edit safety
    files_read: RwLock<HashSet<String>>,
    tools: T,
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
    request_state_codec: rmcp::model::RequestStateCodec,
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

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.intercept_call_tool(request, context).await
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

#[tool_router]
impl<T: CodingTools + 'static> CodingMcp<T> {
    /// Create a `CodingMcp` with custom tool implementation
    pub fn with_tools(tools: T) -> Self {
        Self {
            tool_router: Self::tool_router(),
            background_processes: Mutex::new(HashMap::new()),
            files_read: RwLock::new(HashSet::new()),
            tools,
            lsp: None,
            web_fetcher: WebFetcher::new(),
            web_searcher: WebSearcher::try_new().ok(),
            root_dir: crate::workspace_paths::current_dir(),
            read_rule_state: prompt_rule_matcher::PromptRuleMatcher::default(),
            configured_rules_dirs: Vec::new(),
            permission_mode: PermissionMode::AlwaysAllow,
            request_state_codec: new_request_state_codec(),
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

    /// Tools whose execution can require user approval depending on
    /// `permission_mode`. The manual `call_tool` interception resolves their
    /// permission rounds; everything else is delegated to the tool router.
    fn is_permission_gated(tool_name: &str) -> bool {
        matches!(tool_name, "bash" | "write_file" | "edit_file")
    }

    /// Decide whether `request` needs approval and, when it does, what the
    /// approval prompt should describe. `AlwaysAllow` never reaches this path.
    fn permission_needed(&self, request: &CallToolRequestParams) -> Result<Option<(&'static str, String)>, ErrorData> {
        let tool = request.name.as_ref();
        if self.permission_mode == PermissionMode::AlwaysAsk {
            return match tool {
                "bash" => {
                    let command = tool_argument_string(request, &["command"])?;
                    Ok(Some(("bash", command)))
                }
                "write_file" => {
                    let raw = tool_argument_string(request, &["filePath", "file_path"])?;
                    let path = self.resolve_file_arg(&raw).unwrap_or(raw);
                    Ok(Some(("write_file", path)))
                }
                "edit_file" => {
                    let raw = tool_argument_string(request, &["filePath", "file_path"])?;
                    let path = self.resolve_file_arg(&raw).unwrap_or(raw);
                    Ok(Some(("edit_file", path)))
                }
                _ => Ok(None),
            };
        }
        if self.permission_mode == PermissionMode::Auto && tool == "bash" {
            let command = tool_argument_string(request, &["command"])?;
            if is_dangerous_cmd(&command) {
                return Ok(Some(("bash", command)));
            }
        }
        Ok(None)
    }

    /// First round for a permission-gated tool: open the allow/deny form as an
    /// `InputRequiredResult` and park the approval under an opaque token.
    /// Returns `None` when the call needs no approval and should run directly.
    fn start_permission_round(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<CallToolResponse>, ErrorData> {
        let Some((tool_name, description)) = self.permission_needed(request)? else {
            return Ok(None);
        };
        require_form_elicitation(context)?;
        let form = Self::build_permission_form(tool_name, &description);
        let state = seal_request_state(&self.request_state_codec, request.name.as_ref(), request.arguments.as_ref());
        Ok(Some(input_required(PERMISSION_INPUT_KEY, form, state)))
    }

    /// Retry round for a permission-gated tool: the client echoed
    /// `requestState` and `inputResponses`. `allow` runs the tool through the
    /// router with the original request; `deny`, decline, and cancel surface a
    /// tool-level error. Unknown or mismatched state is a protocol error.
    async fn resolve_permission_round(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let state = request
            .request_state
            .as_deref()
            .ok_or_else(|| ErrorData::invalid_params("permission retry is missing requestState", None))?;
        verify_request_state(&self.request_state_codec, state, request.name.as_ref(), request.arguments.as_ref())?;
        let result = elicitation_result(request.input_responses.as_ref(), PERMISSION_INPUT_KEY)?;

        let approved = match result.action {
            ElicitationAction::Accept => {
                result.content.as_ref().and_then(|c| c.get(PERMISSION_INPUT_KEY)).and_then(serde_json::Value::as_str)
                    == Some("allow")
            }
            ElicitationAction::Decline | ElicitationAction::Cancel => false,
            _ => return Err(ErrorData::invalid_params("permission response carried an unsupported action", None)),
        };

        if approved {
            return route_tool_call(self, &self.tool_router, request, context).await;
        }

        let message = if result.action == ElicitationAction::Cancel {
            format!("Operation cancelled by user: {}", request.name)
        } else {
            format!("Operation declined by user: {}", request.name)
        };
        Ok(CallToolResponse::Complete(CallToolResult::error(vec![ContentBlock::text(message)])))
    }

    fn build_permission_form(tool_name: &str, description: &str) -> ElicitRequestParams {
        let message = format!("Allow {tool_name}: {description}?");
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message,
            requested_schema: ElicitationSchema::builder()
                .required_enum_schema(
                    PERMISSION_INPUT_KEY,
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

        let command = args.command.clone();
        let cwd = self.root_dir.clone();
        let result = self.tools.bash(args, Some(cwd)).await.map_err(|e| e.to_string())?;

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
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
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

impl Default for CodingMcp<DefaultCodingTools> {
    fn default() -> Self {
        Self::new()
    }
}

/// Read one string argument from a tool call, trying `keys` in order.
fn tool_argument_string(request: &CallToolRequestParams, keys: &[&str]) -> Result<String, ErrorData> {
    let arguments = request
        .arguments
        .as_ref()
        .ok_or_else(|| ErrorData::invalid_params(format!("{} requires arguments", request.name), None))?;
    for key in keys {
        if let Some(value) = arguments.get(*key).and_then(serde_json::Value::as_str) {
            return Ok(value.to_string());
        }
    }
    Err(ErrorData::invalid_params(format!("{} is missing required argument '{}'", request.name, keys[0]), None))
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
