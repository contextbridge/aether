use crate::server_config;
use aether_project::PromptCatalog;
use clap::Parser;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::{ToolCallContext, parse_json_object, schema_for_output},
        wrapper::Parameters,
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CreateTaskResult, ElicitRequest,
        ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, EnumSchema, InputRequest,
        InputRequests, InputRequiredResult, ProgressNotificationParam, ServerCapabilities, ServerConfig,
    },
    service::{ElicitationMode, RequestContext},
    tool, tool_handler, tool_router,
};
use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::fmt::{Debug, Formatter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs::try_exists;

pub mod error;
pub mod prompt_rule_matcher;
pub mod tools;

use crate::Displayed;
use crate::background_tasks::{BackgroundTasks, delegate_background_tasks};
use crate::coding::error::CodingError;
use crate::coding::prompt_rule_matcher::PromptRuleMatcher;
use crate::lsp::registry::LspRegistry;
use crate::lsp::tools::check_errors::{LspDiagnosticsOutput, LspDiagnosticsRequest, execute_lsp_diagnostics};
use crate::lsp::tools::rename::{LspRenameInput, LspRenameOutput, execute_lsp_rename};
use crate::lsp::tools::symbol_lookup::{LspSymbolInput, LspSymbolOutput, execute_lsp_symbol};
use crate::request_context::{CLIENT_CANNOT_ELICIT, parse_response, require_tasks, supports_elicitation};
use crate::workspace_paths::{resolve_file, resolve_path, resolve_root};

use tools::bash::{BashEnvironment, BashInput, BashOutput, execute_command, validate_args};
use tools::edit_file::{EditFileArgs, EditFileResponse, edit_file_contents};
use tools::read_file::{ReadFileArgs, ReadFileResult, read_file_contents};
use tools::web_fetch::{WebFetchInput, WebFetchOutput, WebFetcher};
use tools::web_search::{WebSearchInput, WebSearchOutput, WebSearcher};
use tools::write_file::{WriteFileArgs, WriteFileResponse, write_file_contents};
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename, truncate};
use utils::temp_dir::TempDir;
use utils::tool_result_truncator::ToolResultTruncator;

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
#[derive(Debug, Clone, Parser)]
#[command(name = "coding", no_binary_name = true)]
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

#[doc = include_str!("../docs/coding_mcp.md")]
pub struct CodingMcp {
    tool_router: ToolRouter<Self>,
    tasks: BackgroundTasks,
    read_state: Mutex<ReadState>,
    bash_environment: BashEnvironment,
    /// Optional LSP operations (enabled with `.with_lsp()`)
    lsp: Option<Arc<LspRegistry>>,
    web_fetcher: WebFetcher,
    web_searcher: Option<WebSearcher>,
    /// Root directory used for path resolution and tool instructions.
    root_dir: PathBuf,
    /// Permission mode controlling user approval for tool calls
    permission_mode: PermissionMode,
    spill_dir: Arc<TempDir>,
    bash_truncator: ToolResultTruncator,
    web_truncator: ToolResultTruncator,
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for CodingMcp {
    fn get_info(&self) -> ServerConfig {
        server_config(
            "coding-mcp",
            ServerCapabilities::builder().enable_tools().enable_tasks().build(),
            self.build_instructions(),
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let background_args = background_bash_args(&request);
        if let Some(args) = &background_args {
            require_tasks(&context)?;
            validate_args(args).map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
        }

        if let Some(prompt) = self.permission_prompt(&request) {
            let Some(responses) = request.input_responses.clone() else {
                if !supports_elicitation(&context, ElicitationMode::Form) {
                    let message = permission_unsupported(&prompt.tool_name);
                    return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
                }
                let requests = InputRequests::from([(
                    "permission".to_string(),
                    InputRequest::Elicitation(ElicitRequest::new(decision_form(
                        &prompt.tool_name,
                        &prompt.description,
                    ))),
                )]);
                return Ok(InputRequiredResult::from_input_requests(requests).into());
            };

            let result: ElicitResult = parse_response(&responses, "permission")?;
            if !permission_granted(&result) {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "Operation declined by user: {}",
                    prompt.tool_name
                ))])
                .into());
            }
        }
        if let Some(args) = background_args {
            return Ok(CallToolResponse::Task(CreateTaskResult::new(self.create_background_bash_task(args))));
        }

        self.tool_router.call(ToolCallContext::new(self, request, context)).await
    }

    delegate_background_tasks!(tasks);
}

#[tool_router]
impl CodingMcp {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            tool_router: LSP_TOOLS
                .into_iter()
                .chain([WEB_SEARCH_TOOL])
                .fold(Self::tool_router(), ToolRouter::with_disabled),
            tasks: BackgroundTasks::new(),
            read_state: Mutex::new(ReadState::default()),
            bash_environment: BashEnvironment::new().with_current_exe_dir_on_path(),
            lsp: None,
            web_fetcher: WebFetcher::new(),
            web_searcher: None,
            root_dir: root_dir.into(),
            permission_mode: PermissionMode::AlwaysAllow,
            spill_dir: Arc::new(TempDir::new()),
            bash_truncator: ToolResultTruncator { head: 4_000, tail: 12_000 },
            web_truncator: ToolResultTruncator { head: 24_000, tail: 8_000 },
        }
    }

    pub fn from_args(args: CodingMcpArgs, base_dir: &Path) -> Self {
        let CodingMcpArgs { root_dir, rules_dirs, permission_mode, disable_lsp } = args;
        let root_dir = resolve_root(base_dir, root_dir);
        let rules_dirs: Vec<_> = rules_dirs.into_iter().map(|path| resolve_path(&root_dir, path)).collect();
        let mut server = Self::new(root_dir).with_rules_dirs(&rules_dirs).with_permission_mode(permission_mode);
        if let Ok(searcher) = WebSearcher::try_new() {
            server = server.with_web_searcher(searcher);
        }
        if disable_lsp { server } else { server.with_lsp() }
    }

    pub fn with_bash_environment(mut self, environment: BashEnvironment) -> Self {
        self.bash_environment = environment;
        self
    }

    /// Enable LSP code intelligence for the server's root directory.
    ///
    /// LSP servers for detected project languages are spawned immediately
    /// in the background, allowing indexing to start right away.
    pub fn with_lsp(mut self) -> Self {
        self.lsp = Some(LspRegistry::new_and_spawn(self.root_dir.clone()));
        for tool in LSP_TOOLS {
            self.tool_router.enable_route(tool);
        }
        self
    }

    /// Enable the `web_search` tool, backed by `searcher`.
    pub fn with_web_searcher(mut self, searcher: WebSearcher) -> Self {
        self.web_searcher = Some(searcher);
        self.tool_router.enable_route(WEB_SEARCH_TOOL);
        self
    }

    /// Set prompt directories used for read-triggered rule activation.
    pub fn with_rules_dirs(mut self, rules_dirs: &[PathBuf]) -> Self {
        let catalog = PromptCatalog::from_dirs(rules_dirs);
        self.read_state.get_mut().expect("read state lock poisoned").rule_matcher = PromptRuleMatcher::new(catalog);
        self
    }

    /// Set the permission mode controlling user approval for tool calls.
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    #[doc = include_str!("tools/read_file/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<ReadFileResult>()
    )]
    pub async fn read_file(
        &self,
        Parameters(mut args): Parameters<ReadFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<ReadFileResult>, CodingError> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Read file", basename(&args.file_path))).await;
        self.read_and_track(args).await
    }

    #[doc = include_str!("tools/write_file/description.md")]
    #[tool(
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = schema_for_output::<WriteFileResponse>()
    )]
    pub async fn write_file(
        &self,
        Parameters(mut args): Parameters<WriteFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<WriteFileResponse>, CodingError> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Write file", basename(&args.file_path))).await;

        self.ensure_read_before_overwrite(&args.file_path).await?;

        let response = write_file_contents(args).await?;

        self.spawn_diagnostic_refresh(&response.output.file_path);

        Ok(response)
    }

    #[doc = include_str!("tools/edit_file/description.md")]
    #[tool(
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = schema_for_output::<EditFileResponse>()
    )]
    pub async fn edit_file(
        &self,
        Parameters(mut args): Parameters<EditFileArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<EditFileResponse>, CodingError> {
        args.file_path = self.resolve_file_arg(&args.file_path)?;
        notify_preview(&context, ToolDisplayMeta::new("Edit file", basename(&args.file_path))).await;

        self.ensure_read_before_edit(&args.file_path)?;

        let response = edit_file_contents(args).await?;

        self.spawn_diagnostic_refresh(&response.output.file_path);

        Ok(response)
    }

    #[doc = include_str!("tools/bash/description.md")]
    #[tool(
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true),
        output_schema = schema_for_output::<BashOutput>()
    )]
    pub async fn bash(
        &self,
        Parameters(args): Parameters<BashInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<BashOutput>, CodingError> {
        notify_preview(&context, ToolDisplayMeta::new("Run command", truncate(&args.command, 40))).await;

        run_bash(args, &self.root_dir, &self.bash_environment, self.bash_truncator, &self.spill_dir).await
    }

    #[doc = include_str!("tools/web_fetch/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = true),
        output_schema = schema_for_output::<WebFetchOutput>()
    )]
    pub async fn web_fetch(
        &self,
        Parameters(args): Parameters<WebFetchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<WebFetchOutput>, CodingError> {
        notify_preview(&context, ToolDisplayMeta::new("Fetch URL", truncate(&args.url, 60))).await;
        let output = self.web_fetcher.fetch(args).await?;
        let page = output.title.clone().unwrap_or_else(|| truncate(&output.final_url, 60));
        let content = self.web_truncator.truncate(output.content, &self.spill_dir, "web-fetch");
        Ok(Displayed::new(WebFetchOutput { content, ..output }, ToolDisplayMeta::new("Fetch URL", page)))
    }

    #[doc = include_str!("tools/web_search/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = true),
        output_schema = schema_for_output::<WebSearchOutput>()
    )]
    pub async fn web_search(
        &self,
        Parameters(args): Parameters<WebSearchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<WebSearchOutput>, CodingError> {
        notify_preview(&context, ToolDisplayMeta::new("Web search", format!("'{}'", args.query))).await;

        let searcher = self.web_searcher.as_ref().ok_or(CodingError::NotConfigured("Web search"))?;
        let output = searcher
            .search(args, |delay| {
                notify(&context, format!("Search rate limited; retrying in {} seconds.", delay.as_secs()))
            })
            .await?;
        let summary = format!("'{}' ({} results)", output.query, output.results.len());
        Ok(Displayed::new(output, ToolDisplayMeta::new("Web search", summary)))
    }

    #[doc = include_str!("../lsp/tools/symbol_lookup/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<LspSymbolOutput>()
    )]
    pub async fn lsp_symbol(
        &self,
        Parameters(input): Parameters<LspSymbolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<LspSymbolOutput>, CodingError> {
        notify_preview(&context, ToolDisplayMeta::new("LSP symbol", &input.symbol)).await;
        execute_lsp_symbol(input, self.lsp()?).await.map_err(CodingError::from)
    }

    #[doc = include_str!("../lsp/tools/check_errors/description.md")]
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<LspDiagnosticsOutput>()
    )]
    pub async fn lsp_check_errors(
        &self,
        Parameters(request): Parameters<LspDiagnosticsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<LspDiagnosticsOutput>, CodingError> {
        let preview_value = request.file_path.as_ref().map_or_else(|| "workspace".to_string(), |path| basename(path));
        notify_preview(&context, ToolDisplayMeta::new("LSP errors", preview_value)).await;
        execute_lsp_diagnostics(request, self.lsp()?).await.map_err(CodingError::from)
    }

    #[doc = include_str!("../lsp/tools/rename/description.md")]
    #[tool(
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false),
        output_schema = schema_for_output::<LspRenameOutput>()
    )]
    pub async fn lsp_rename(
        &self,
        Parameters(input): Parameters<LspRenameInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Displayed<LspRenameOutput>, CodingError> {
        notify_preview(&context, ToolDisplayMeta::new("LSP rename", &input.symbol)).await;
        execute_lsp_rename(input, self.lsp()?).await.map_err(CodingError::from)
    }
}

#[cfg(feature = "test-helpers")]
impl CodingMcp {
    pub fn with_web_fetcher(mut self, web_fetcher: WebFetcher) -> Self {
        self.web_fetcher = web_fetcher;
        self
    }
}

impl CodingMcp {
    fn create_background_bash_task(&self, args: BashInput) -> rmcp::model::Task {
        let cwd = self.root_dir.clone();
        let environment = self.bash_environment.clone();
        let spill_dir = Arc::clone(&self.spill_dir);
        let truncator = self.bash_truncator;
        let description = args.description.clone().unwrap_or_else(|| truncate(&args.command, 80));

        self.tasks.spawn(description, async move {
            let result = run_bash(args, &cwd, &environment, truncator, &spill_dir).await.map_err(|e| e.to_string());
            let result = result.and_then(|output| output.into_result().map_err(|e| e.to_string()));
            Ok(result.unwrap_or_else(|error| CallToolResult::error(vec![ContentBlock::text(error)])))
        })
    }

    fn build_instructions(&self) -> String {
        let mut base = String::from(
            r"# Coding MCP Server

File I/O, shell, and optional LSP code intelligence tools for coding workflows.

## Quick Reference

- **Search, symbol lookup by name, file outlines, and directory listings**: run `rg`, `ast-grep`, and other shell commands with `bash`
- **Read/write/edit** files: `read_file`, `write_file`, `edit_file`
- **Shell commands**: `bash`
",
        );

        if self.lsp.is_some() {
            base.push_str(
                r"- **Errors & warnings** (instant check without build): `lsp_check_errors`
- **Code symbols** (definitions, usages, types): `lsp_symbol`
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
        if !destructive || self.permission_mode == PermissionMode::AlwaysAllow {
            return None;
        }

        let description = match name {
            "bash" => parse_arguments::<BashInput>(request)?.command,
            "write_file" | "edit_file" => {
                let file_path = parse_arguments::<FilePathArg>(request)?.file_path;
                self.resolve_file_arg(&file_path).unwrap_or(file_path)
            }
            _ => {
                let args = request.arguments.as_ref().and_then(|a| serde_json::to_string(a).ok());
                truncate(&args.unwrap_or_default(), 80)
            }
        };
        let gated =
            self.permission_mode == PermissionMode::AlwaysAsk || (name == "bash" && is_dangerous_cmd(&description));
        gated.then(|| PermissionPrompt { tool_name: name.to_string(), description })
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
    fn resolve_file_arg(&self, raw: &str) -> Result<String, CodingError> {
        let path = resolve_file(&self.root_dir, raw)?;
        Ok(path.to_string_lossy().to_string())
    }

    fn lsp(&self) -> Result<&LspRegistry, CodingError> {
        self.lsp.as_deref().ok_or(CodingError::NotConfigured("LSP"))
    }

    /// Reads `args.file_path` (already resolved against the root directory),
    /// records it in the read set, and appends any matching read-rule reminders.
    async fn read_and_track(&self, args: ReadFileArgs) -> Result<Displayed<ReadFileResult>, CodingError> {
        let file_path = args.file_path.clone();
        let mut result = read_file_contents(args).await?;
        let matched = {
            let mut state = self.read_state.lock().expect("read state lock poisoned");
            state.files_read.insert(file_path.clone());
            state.rule_matcher.get_matched_rules(&self.root_dir, &file_path)
        };

        for rule in &matched {
            write!(result.content, "\n\n<system-reminder>\n{}\n</system-reminder>", rule.body).unwrap();
        }

        let mut summary = format!("{}, {} lines", basename(&file_path), result.total_lines);
        if !matched.is_empty() {
            let rule_names: Vec<&str> = matched.iter().map(|r| r.name.as_str()).collect();
            write!(summary, " +rules: {}", rule_names.join(", ")).unwrap();
        }

        Ok(Displayed::new(result, ToolDisplayMeta::new("Read file", summary)))
    }

    /// Read-before-overwrite safety check: an existing file must have been read
    /// first, preventing accidental data loss.
    async fn ensure_read_before_overwrite(&self, file_path: &str) -> Result<(), CodingError> {
        let exists = try_exists(file_path)
            .await
            .map_err(|source| CodingError::ExistsCheckFailed { path: file_path.to_string(), source })?;
        if exists && !self.read_state.lock().expect("read state lock poisoned").files_read.contains(file_path) {
            return Err(CodingError::NotReadBeforeOverwrite(file_path.to_string()));
        }
        Ok(())
    }

    /// Read-before-edit safety check: a file must have been read before editing.
    fn ensure_read_before_edit(&self, file_path: &str) -> Result<(), CodingError> {
        if !self.read_state.lock().expect("read state lock poisoned").files_read.contains(file_path) {
            return Err(CodingError::NotReadBeforeEdit(file_path.to_string()));
        }
        Ok(())
    }
}

impl Debug for CodingMcp {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodingMcp")
            .field("root_dir", &self.root_dir)
            .field("permission_mode", &self.permission_mode)
            .finish_non_exhaustive()
    }
}

const LSP_TOOLS: [&str; 3] = ["lsp_check_errors", "lsp_rename", "lsp_symbol"];
const WEB_SEARCH_TOOL: &str = "web_search";

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

#[derive(Default)]
struct ReadState {
    files_read: HashSet<String>,
    rule_matcher: PromptRuleMatcher,
}

async fn notify_preview(context: &RequestContext<RoleServer>, meta: ToolDisplayMeta) {
    notify(context, serde_json::to_string(&ToolResultMeta::from(meta)).unwrap_or_default()).await;
}

async fn notify(context: &RequestContext<RoleServer>, message: String) {
    if let Some(token) = context.meta.get_progress_token() {
        let _ = context.peer.notify_progress(ProgressNotificationParam::new(token, 0.0).with_message(message)).await;
    }
}

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

    if PATTERNS.iter().any(|p| command.contains(p)) {
        return true;
    }

    for (i, _) in command.match_indices("> ") {
        if i == 0 || command.as_bytes()[i - 1] != b'=' {
            return true;
        }
    }

    false
}

fn permission_granted(result: &ElicitResult) -> bool {
    result.action == ElicitationAction::Accept
        && result.content.as_ref().and_then(|c| c.get("decision")).and_then(|v| v.as_str()) == Some("allow")
}

fn permission_unsupported(tool_name: &str) -> String {
    format!(
        "Permission required for {tool_name}, but {CLIENT_CANNOT_ELICIT}. \
         Use --permission-mode always-allow or an elicitation-capable client."
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

async fn run_bash(
    args: BashInput,
    cwd: &Path,
    environment: &BashEnvironment,
    truncator: ToolResultTruncator,
    spill_dir: &TempDir,
) -> Result<Displayed<BashOutput>, CodingError> {
    let command = truncate(&args.command, 40);
    let output = execute_command(args, Some(cwd), environment).await?;
    let timed_out = if output.killed { ", timed out" } else { "" };
    let meta = ToolDisplayMeta::new("Ran", format!("{command} (exit {}{timed_out})", output.exit_code));
    Ok(Displayed::new(BashOutput { output: truncator.truncate(output.output, spill_dir, "bash"), ..output }, meta))
}

fn background_bash_args(request: &CallToolRequestParams) -> Option<BashInput> {
    if request.name.as_ref() != "bash" {
        return None;
    }
    parse_arguments::<BashInput>(request).filter(|args| args.run_in_background == Some(true))
}

fn parse_arguments<T: DeserializeOwned>(request: &CallToolRequestParams) -> Option<T> {
    request.arguments.clone().and_then(|arguments| parse_json_object(arguments).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse_permission_mode_lsp_and_rules_dirs() {
        let cases: [(&[&str], PermissionMode, bool); 5] = [
            (&[], PermissionMode::AlwaysAllow, false),
            (&["--permission-mode", "always-allow"], PermissionMode::AlwaysAllow, false),
            (&["--permission-mode", "auto"], PermissionMode::Auto, false),
            (&["--permission-mode", "always-ask"], PermissionMode::AlwaysAsk, false),
            (&["--disable-lsp"], PermissionMode::AlwaysAllow, true),
        ];
        for (argv, permission_mode, disable_lsp) in cases {
            let args = CodingMcpArgs::try_parse_from(argv).unwrap();
            assert_eq!((args.permission_mode, args.disable_lsp), (permission_mode, disable_lsp), "{argv:?}");
        }

        assert!(CodingMcpArgs::try_parse_from(["--permission-mode", "yolo"]).is_err());
        let args = CodingMcpArgs::try_parse_from(["--rules-dir", ".aether/skills", "--rules-dir", ".claude/rules"]);
        assert_eq!(args.unwrap().rules_dirs, [PathBuf::from(".aether/skills"), PathBuf::from(".claude/rules")]);
    }
}
