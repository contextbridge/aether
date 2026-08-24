use crate::error::ServerInitError;
use crate::file_ops::{FileEdit, FileError, apply_edits, read_text_file, write_text_file};
use crate::workspace_paths::resolve_path;
use clap::Parser;
use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta, basename};
use mcp_utils::server::mrtr::{ELICITATION_UNSUPPORTED, input_requests_supported, parse_response};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::{InputResponses, IntoCallToolResult, RequestState, schema_for_output},
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolResponse, CallToolResult, ContentBlock, ElicitRequest, ElicitRequestParams, ElicitResult,
        ElicitationAction, ElicitationSchema, EnumSchema, GetPromptRequestParams, GetPromptResponse, Implementation,
        InputRequest, InputRequests, InputRequiredResult, ListPromptsResult, PaginatedRequestParams, Prompt,
        PromptArgument, PromptMessage, RequestMetaObject, Role, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env::current_dir;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs::read_to_string;
use tokio::process::Command;
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};
use utils::substitution::substitute_parameters;

pub const DEFAULT_PLAN_PROMPT: &str = include_str!("./default_prompt.md");
pub const DEFAULT_PLANS_DIR: &str = "docs/aether/plans";

const DECISION: &str = "decision";
const FEEDBACK: &str = "feedback";
const PROMPT_NAME: &str = "plan";
const ARGUMENTS: &str = "ARGUMENTS";

#[derive(Debug, Clone, Parser)]
#[command(name = "plan-mcp")]
pub struct PlanMcpArgs {
    /// Directory where plan markdown files are written and read.
    /// Defaults to `docs/aether/plans` relative to the server process cwd.
    #[arg(long)]
    pub plans_dir: Option<PathBuf>,

    /// Markdown file whose body is returned as the `plan` MCP prompt.
    /// When the flag is absent or the file is missing at invocation time,
    /// `DEFAULT_PLAN_PROMPT` is used instead.
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,

    /// Command invoked instead of the default MCP elicitation when
    /// `submit_plan` is called. All trailing positional tokens in the
    /// `mcp.json` `args` array become the program + its arguments; the
    /// absolute plan-file path is appended as the final positional arg.
    /// Stdout from the command is returned verbatim to the agent as
    /// feedback.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub submit_command: Vec<String>,
}

impl PlanMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["plan-mcp".to_string()];
        full_args.extend(args);
        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }
}

#[doc = include_str!("../docs/plan_mcp.md")]
#[derive(Clone)]
pub struct PlanMcp {
    tool_router: ToolRouter<Self>,
    plans_dir: PathBuf,
    prompt_file: Option<PathBuf>,
    submit_command: Vec<String>,
}

#[tool_router]
impl PlanMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            plans_dir: default_plans_dir(),
            prompt_file: None,
            submit_command: Vec::new(),
        }
    }

    /// Builds a server from raw `mcp.json` args. `default_plans_dir` is used
    /// when the caller did not pass `--plans-dir`; it lets the embedder anchor
    /// plans at a workspace path that differs from the process cwd.
    pub fn from_args(args: Vec<String>, default_plans_dir: PathBuf) -> Result<Self, ServerInitError> {
        let PlanMcpArgs { plans_dir, prompt_file, submit_command } = PlanMcpArgs::from_args(args)?;
        Ok(Self {
            tool_router: Self::tool_router(),
            plans_dir: absolutize(plans_dir.unwrap_or(default_plans_dir)),
            prompt_file,
            submit_command,
        })
    }

    pub fn from_args_with_base_dir(
        args: Vec<String>,
        default_plans_dir: PathBuf,
        base_dir: &Path,
    ) -> Result<Self, ServerInitError> {
        let PlanMcpArgs { plans_dir, prompt_file, submit_command } = PlanMcpArgs::from_args(args)?;
        Ok(Self {
            tool_router: Self::tool_router(),
            plans_dir: plans_dir.map_or(default_plans_dir, |path| resolve_path(base_dir, path)),
            prompt_file: prompt_file.map(|path| resolve_path(base_dir, path)),
            submit_command,
        })
    }

    pub fn with_plans_dir(mut self, path: PathBuf) -> Self {
        self.plans_dir = absolutize(path);
        self
    }

    pub fn with_prompt_file(mut self, path: PathBuf) -> Self {
        self.prompt_file = Some(path);
        self
    }

    pub fn with_submit_command(mut self, command: Vec<String>) -> Self {
        self.submit_command = command;
        self
    }

    #[doc = include_str!("./write_plan_description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn write_plan(&self, request: Parameters<WritePlanInput>) -> Result<Json<WritePlanOutput>, String> {
        let Parameters(input) = request;
        write_plan_file(&self.plans_dir, input).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("./edit_plan_description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn edit_plan(&self, request: Parameters<EditPlanInput>) -> Result<Json<EditPlanOutput>, String> {
        let Parameters(input) = request;
        edit_plan_file(&self.plans_dir, input).await.map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("./submit_plan_description.md")]
    #[tool(
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true),
        output_schema = schema_for_output::<SubmitPlanOutput>()
    )]
    pub async fn submit_plan(
        &self,
        request: Parameters<SubmitPlanInput>,
        responses: InputResponses,
        _request_state: RequestState,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Parameters(input) = request;
        let plan = match Plan::load_by_name(&self.plans_dir, &input.plan_name).await {
            Ok(plan) => plan,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]).into()),
        };

        if !self.submit_command.is_empty() {
            return match run_external_submit(&plan, &self.submit_command).await {
                Ok(output) => Ok(Json(output).into_call_tool_result()?),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]).into()),
            };
        }

        let Some(responses) = responses.0 else {
            let params = Self::build_elicitation_form(&plan).map_err(|e| McpError::internal_error(e, None))?;
            let requests =
                InputRequests::from([("review".to_string(), InputRequest::Elicitation(ElicitRequest::new(params)))]);
            if !input_requests_supported(context.client_capabilities().as_ref(), &requests) {
                return Ok(CallToolResult::error(vec![ContentBlock::text(ELICITATION_UNSUPPORTED)]).into());
            }
            return Ok(InputRequiredResult::from_input_requests(requests).into());
        };
        let result: ElicitResult = parse_response(&responses, "review")?;

        Json(review_decision(&result)).into_call_tool_result()
    }

    fn build_elicitation_form(plan: &Plan) -> Result<ElicitRequestParams, String> {
        let meta = PlanReviewElicitationMeta::new(&plan.path, &plan.content)
            .to_json()
            .map(RequestMetaObject::from)
            .map_err(|e| format!("failed to serialize plan review metadata: {e}"))?;

        let approve = PlanReviewDecision::Approve.as_str();
        let deny = PlanReviewDecision::Deny.as_str();
        let decision_schema = EnumSchema::builder(vec![approve.into(), deny.into()])
            .untitled()
            .with_default(deny)
            .map_err(|e| format!("failed to build decision schema: {e}"))?
            .build();

        Ok(ElicitRequestParams::FormElicitationParams {
            meta: Some(meta),
            message: format!("Approve plan {}? Review the markdown and choose approve or deny.", plan.path.display()),
            requested_schema: ElicitationSchema::builder()
                .required_enum_schema(DECISION, decision_schema)
                .optional_string(FEEDBACK)
                .build()
                .map_err(|e| format!("failed to build schema: {e}"))?,
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for PlanMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().enable_tools().build())
            .with_server_info(Implementation::new("plan-mcp", "0.1.0"))
            .with_instructions("MCP Server for Plan mode")
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompt = Prompt::new(
            PROMPT_NAME,
            Some("Generate an implementation plan for a task.".to_string()),
            Some(vec![
                PromptArgument::new(ARGUMENTS)
                    .with_description("The task to generate a plan for.".to_string())
                    .with_required(true),
            ]),
        );

        Ok(ListPromptsResult::with_all_items(vec![prompt]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        if request.name.as_str() != PROMPT_NAME {
            return Err(McpError::invalid_params(format!("Prompt '{}' not found", request.name), None));
        }

        let prompt = match &self.prompt_file {
            Some(path) => read_to_string(path).await.unwrap_or(DEFAULT_PLAN_PROMPT.to_string()),
            None => DEFAULT_PLAN_PROMPT.to_string(),
        };

        let arguments: Option<HashMap<String, String>> = request.arguments.as_ref().map(|json_map| {
            json_map.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()
        });

        let content = substitute_parameters(&prompt, &arguments);
        let messages = vec![PromptMessage::new_text(Role::User, content)];
        Ok(rmcp::model::GetPromptResult::new(messages).with_description("Enter plan mode.".to_string()).into())
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WritePlanInput {
    /// Stable plan identifier chosen by the agent, e.g. `auth-refactor`.
    #[serde(alias = "planName")]
    pub plan_name: String,
    /// Markdown plan body to write.
    pub content: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WritePlanOutput {
    pub plan_name: String,
    pub plan_path: String,
    pub bytes_written: usize,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditPlanInput {
    /// Stable plan identifier originally passed to `write_plan`.
    #[serde(alias = "planName")]
    pub plan_name: String,
    /// Exact-string replacements to apply, in one atomic batch. All edits must
    /// apply or none are written.
    pub edits: Vec<FileEdit>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditPlanOutput {
    pub plan_name: String,
    pub plan_path: String,
    pub replacements_made: usize,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPlanInput {
    /// Stable plan identifier originally passed to `write_plan`.
    #[serde(alias = "planName")]
    pub plan_name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPlanOutput {
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("Invalid planName `{0}`; use only letters, numbers, dashes, and underscores")]
    InvalidPlanName(String),

    #[error(transparent)]
    File(#[from] FileError),

    #[error("Plan file is empty: {}", .0.display())]
    EmptyPlan(PathBuf),

    #[error("submit_command is empty; expected at least a program name")]
    EmptySubmitCommand,

    #[error("Failed to spawn submit command `{program}`: {source}")]
    SubmitCommandSpawn { program: String, source: std::io::Error },

    #[error("Submit command `{program}` exited with {status}{}", stderr_suffix(.stderr))]
    SubmitCommandFailed { program: String, status: std::process::ExitStatus, stderr: String },
}

struct PlanName(String);

struct Plan {
    path: PathBuf,
    content: String,
}

impl PlanName {
    fn parse(value: &str) -> Result<Self, PlanError> {
        if Self::is_valid_plan_name(value) {
            Ok(Self(value.to_string()))
        } else {
            Err(PlanError::InvalidPlanName(value.to_string()))
        }
    }

    fn into_string(self) -> String {
        self.0
    }

    fn file_name(&self) -> String {
        format!("{}-plan.md", self.0)
    }

    fn is_valid_plan_name(value: &str) -> bool {
        let is_boundary = |c: char| c == '-' || c == '_';
        !value.is_empty()
            && value.chars().all(|c| c.is_ascii_alphanumeric() || is_boundary(c))
            && !value.starts_with(is_boundary)
            && !value.ends_with(is_boundary)
    }
}

impl Plan {
    async fn load_by_name(plans_dir: &Path, plan_name: &str) -> Result<Self, PlanError> {
        let plan_name = PlanName::parse(plan_name)?;
        let path = plan_path(plans_dir, &plan_name);
        let content = read_text_file(&path).await?;
        if content.trim().is_empty() {
            return Err(PlanError::EmptyPlan(path));
        }

        Ok(Self { path, content })
    }
}

impl Default for PlanMcp {
    fn default() -> Self {
        Self::new()
    }
}

async fn write_plan_file(plans_dir: &Path, input: WritePlanInput) -> Result<WritePlanOutput, PlanError> {
    let plan_name = PlanName::parse(&input.plan_name)?;
    let path = plan_path(plans_dir, &plan_name);
    if input.content.trim().is_empty() {
        return Err(PlanError::EmptyPlan(path));
    }

    let result = write_text_file(&path, &input.content).await?;
    let plan_path = result.path.to_string_lossy().into_owned();
    let display_meta = ToolDisplayMeta::new("Write plan", basename(&plan_path));
    let file_diff = FileDiff { path: plan_path.clone(), old_text: None, new_text: input.content };

    Ok(WritePlanOutput {
        plan_name: plan_name.into_string(),
        plan_path,
        bytes_written: result.bytes_written,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

async fn edit_plan_file(plans_dir: &Path, input: EditPlanInput) -> Result<EditPlanOutput, PlanError> {
    let plan_name = PlanName::parse(&input.plan_name)?;
    let path = plan_path(plans_dir, &plan_name);
    let result = apply_edits(&path, &input.edits).await?;
    let plan_path = result.path.to_string_lossy().into_owned();
    let display_meta = ToolDisplayMeta::new("Edit plan", basename(&plan_path));
    let file_diff =
        FileDiff { path: plan_path.clone(), old_text: Some(result.original_content), new_text: result.updated_content };

    Ok(EditPlanOutput {
        plan_name: plan_name.into_string(),
        plan_path,
        replacements_made: result.replacements_made,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

pub fn default_plans_dir() -> PathBuf {
    absolutize(PathBuf::from(DEFAULT_PLANS_DIR))
}

fn plan_path(plans_dir: &Path, plan_name: &PlanName) -> PathBuf {
    plans_dir.join(plan_name.file_name())
}

fn absolutize(path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path) }
}

fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() { String::new() } else { format!(": {trimmed}") }
}

fn review_decision(result: &ElicitResult) -> SubmitPlanOutput {
    if result.action != ElicitationAction::Accept {
        return SubmitPlanOutput { approved: false, feedback: None };
    }

    let decision = result
        .content
        .as_ref()
        .and_then(|content| content.get(DECISION))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(PlanReviewDecision::Deny.as_str());

    if decision == PlanReviewDecision::Approve.as_str() {
        return SubmitPlanOutput { approved: true, feedback: None };
    }

    let feedback = result
        .content
        .as_ref()
        .and_then(|content| content.get(FEEDBACK))
        .and_then(serde_json::Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    SubmitPlanOutput { approved: false, feedback }
}

async fn run_external_submit(plan: &Plan, command: &[String]) -> Result<SubmitPlanOutput, PlanError> {
    let (program, extra_args) = command.split_first().ok_or(PlanError::EmptySubmitCommand)?;
    let output = Command::new(program)
        .args(extra_args)
        .arg(&plan.path)
        .output()
        .await
        .map_err(|source| PlanError::SubmitCommandSpawn { program: program.clone(), source })?;

    if !output.status.success() {
        return Err(PlanError::SubmitCommandFailed {
            program: program.clone(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(SubmitPlanOutput { approved: false, feedback: Some(stdout) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DEFAULT: &str = "/workspace/docs/aether/plans";

    fn default() -> PathBuf {
        PathBuf::from(TEST_DEFAULT)
    }

    #[test]
    fn from_args_parses_prompt_file() {
        let server = PlanMcp::from_args(vec!["--prompt-file".into(), "/tmp/plan.md".into()], default()).unwrap();
        assert_eq!(server.prompt_file, Some(PathBuf::from("/tmp/plan.md")));
    }

    #[test]
    fn from_args_uses_default_plans_dir_when_flag_absent() {
        let server = PlanMcp::from_args(vec![], default()).unwrap();
        assert_eq!(server.plans_dir, default());
        assert_eq!(server.prompt_file, None);
        assert!(server.submit_command.is_empty());
    }

    #[test]
    fn from_args_explicit_plans_dir_overrides_default() {
        let server = PlanMcp::from_args(vec!["--plans-dir".into(), "/tmp/plans".into()], default()).unwrap();
        assert_eq!(server.plans_dir, PathBuf::from("/tmp/plans"));
    }

    #[test]
    fn from_args_with_base_dir_resolves_relative_paths() {
        let workspace = PathBuf::from("/workspace");
        let server = PlanMcp::from_args_with_base_dir(
            vec!["--plans-dir".into(), "plans".into(), "--prompt-file".into(), "prompts/plan.md".into()],
            default(),
            &workspace,
        )
        .unwrap();

        assert_eq!(server.plans_dir, PathBuf::from("/workspace/plans"));
        assert_eq!(server.prompt_file, Some(PathBuf::from("/workspace/prompts/plan.md")));
    }

    #[test]
    fn from_args_with_base_dir_keeps_absolute_paths() {
        let workspace = PathBuf::from("/workspace");
        let server = PlanMcp::from_args_with_base_dir(
            vec!["--plans-dir".into(), "/tmp/plans".into(), "--prompt-file".into(), "/tmp/plan.md".into()],
            default(),
            &workspace,
        )
        .unwrap();

        assert_eq!(server.plans_dir, PathBuf::from("/tmp/plans"));
        assert_eq!(server.prompt_file, Some(PathBuf::from("/tmp/plan.md")));
    }

    #[test]
    fn from_args_keeps_submit_command_separate_from_plans_dir() {
        let server = PlanMcp::from_args(
            vec!["contextbridge".into(), "plan".into(), "--plans-dir".into(), "external".into()],
            default(),
        )
        .unwrap();

        assert_eq!(server.plans_dir, default());
        assert_eq!(server.submit_command, vec!["contextbridge", "plan", "--plans-dir", "external"]);
    }

    #[test]
    fn from_args_parses_trailing_submit_command() {
        let server = PlanMcp::from_args(
            vec!["contextbridge".into(), "plan".into(), "--project".into(), "foo".into()],
            default(),
        )
        .unwrap();
        assert_eq!(server.submit_command, vec!["contextbridge", "plan", "--project", "foo"]);
    }

    #[test]
    fn from_args_parses_prompt_file_followed_by_submit_command() {
        let server = PlanMcp::from_args(
            vec!["--prompt-file".into(), "/tmp/plan.md".into(), "contextbridge".into(), "plan".into()],
            default(),
        )
        .unwrap();
        assert_eq!(server.prompt_file, Some(PathBuf::from("/tmp/plan.md")));
        assert_eq!(server.submit_command, vec!["contextbridge", "plan"]);
    }
}
