use crate::mrtr::PendingRequestTable;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::ToolCallContext,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ElicitRequest, ElicitRequestParams, ElicitResult,
        ElicitationAction, ElicitationSchema, ErrorData, Implementation, InputRequest, InputRequests,
        InputRequiredResult, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, from_str, from_value};

const ASK_USER_TOOL: &str = "ask_user";
const INPUT_KEY: &str = "form";

/// Parse a schema value that may be either a JSON object or a double-encoded JSON string.
fn parse_schema(value: serde_json::Value) -> Result<ElicitationSchema, serde_json::Error> {
    let normalized = match &value {
        Value::String(s) => from_str(s)?,
        _ => value,
    };

    from_value(normalized)
}

#[doc = include_str!("../docs/survey_mcp.md")]
#[derive(Clone)]
pub struct SurveyMcp {
    tool_router: ToolRouter<Self>,
    pending: PendingRequestTable<PendingSurvey>,
}

/// Continuation state for one in-flight `ask_user` MRTR interaction.
#[derive(Clone)]
struct PendingSurvey {
    message: String,
    schema: ElicitationSchema,
}

impl Default for SurveyMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl SurveyMcp {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router(), pending: PendingRequestTable::new() }
    }

    pub fn from_args(_args: Vec<String>) -> Result<Self, crate::error::ServerInitError> {
        Ok(Self::new())
    }

    /// First round: parse the requested schema, open the form as an
    /// `InputRequiredResult`, and park the continuation under an opaque token.
    async fn start_ask_user_round(&self, request: CallToolRequestParams) -> Result<CallToolResponse, ErrorData> {
        let arguments =
            request.arguments.ok_or_else(|| ErrorData::invalid_params("ask_user requires arguments", None))?;
        let args: AskUserInput = serde_json::from_value(Value::Object(arguments))
            .map_err(|error| ErrorData::invalid_params(format!("failed to deserialize parameters: {error}"), None))?;
        let schema = parse_schema(args.schema)
            .map_err(|error| ErrorData::invalid_params(format!("invalid schema: {error}"), None))?;

        Ok(self.require_input(args.message, schema).await)
    }

    /// Retry round: the client echoed `requestState` and `inputResponses`.
    /// Accept completes the survey; decline and cancel report `accepted:
    /// false`; invalid state is a protocol error; content that fails basic
    /// schema validation opens one more input round.
    async fn resolve_ask_user_round(
        &self,
        token: &str,
        input_responses: Option<rmcp::model::InputResponses>,
    ) -> Result<CallToolResponse, ErrorData> {
        let pending = self.pending.take(token).await.ok_or_else(|| {
            ErrorData::invalid_params("ask_user retry carried an unknown or expired requestState", None)
        })?;
        let result = elicitation_result(input_responses, INPUT_KEY)?;

        match result.action {
            ElicitationAction::Accept => {
                let Some(content) = result.content else {
                    return Ok(CallToolResponse::Complete(CallToolResult::structured(serde_json::json!({
                        "accepted": false,
                        "data": Value::Null,
                    }))));
                };
                if validate_against_schema(&pending.schema, &content).is_err() {
                    return Ok(self.require_input(pending.message, pending.schema).await);
                }
                Ok(CallToolResponse::Complete(CallToolResult::structured(serde_json::json!({
                    "accepted": true,
                    "data": content,
                }))))
            }
            ElicitationAction::Decline | ElicitationAction::Cancel => {
                Ok(CallToolResponse::Complete(CallToolResult::structured(serde_json::json!({
                    "accepted": false,
                    "data": Value::Null,
                }))))
            }
            _ => Err(ErrorData::invalid_params("ask_user response carried an unsupported action", None)),
        }
    }

    async fn require_input(&self, message: String, schema: ElicitationSchema) -> CallToolResponse {
        let token = self.pending.insert(PendingSurvey { message: message.clone(), schema: schema.clone() }).await;
        let mut requests = InputRequests::new();
        requests.insert(
            INPUT_KEY.to_string(),
            InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
                meta: None,
                message,
                requested_schema: schema,
            })),
        );
        CallToolResponse::InputRequired(InputRequiredResult::new(Some(requests), Some(token)))
    }
}

/// Input for the `ask_user` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskUserInput {
    /// The question or prompt to show the user.
    pub message: String,
    /// JSON Schema describing the form fields to present.
    /// Must be an object schema with `properties`.
    pub schema: serde_json::Value,
}

/// Output from the `ask_user` tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AskUserOutput {
    /// Whether the user accepted (true) or declined/cancelled (false).
    pub accepted: bool,
    /// The structured data from the user, if accepted.
    pub data: Option<serde_json::Value>,
}

#[tool_router]
impl SurveyMcp {
    /// Ask the user a structured question and collect their response via a form.
    ///
    /// Use this to gather information from the user when you need specific inputs
    /// (text, numbers, booleans, selections). The schema parameter defines the form
    /// fields using JSON Schema format.
    ///
    /// The interaction is driven through MRTR `InputRequiredResult` rounds; the
    /// router path below is never reached because `ServerHandler::call_tool`
    /// intercepts `ask_user` and resolves the rounds itself.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = true
    ))]
    pub async fn ask_user(
        &self,
        _request: Parameters<AskUserInput>,
        _context: RequestContext<RoleServer>,
    ) -> Result<Json<AskUserOutput>, String> {
        Err("ask_user is resolved through MRTR input rounds and cannot run directly".to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SurveyMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("survey-mcp", "0.1.0"))
            .with_instructions(
                "Ask the user structured questions using the `ask_user` tool. \
                 Define form schemas to collect text, numbers, booleans, and selections.",
            )
    }

    /// Manual interception for `ask_user`: rmcp's tool router drops
    /// `input_responses`/`request_state` when it builds `ToolCallContext`, so
    /// the MRTR rounds are resolved here and every other tool is delegated to
    /// the generated router untouched.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() == ASK_USER_TOOL {
            return match request.request_state.as_deref() {
                Some(token) => self.resolve_ask_user_round(token, request.input_responses).await,
                None if request.input_responses.is_some() => {
                    Err(ErrorData::invalid_params("ask_user retry carried inputResponses without requestState", None))
                }
                None => self.start_ask_user_round(request).await,
            };
        }
        let tool_call_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_call_context).await
    }
}

/// Extract the client's `ElicitResult` for `key` from an MRTR retry.
fn elicitation_result(
    input_responses: Option<rmcp::model::InputResponses>,
    key: &str,
) -> Result<ElicitResult, ErrorData> {
    let responses = input_responses
        .ok_or_else(|| ErrorData::invalid_params("ask_user retry carried requestState without inputResponses", None))?;
    let value = responses
        .get(key)
        .cloned()
        .ok_or_else(|| ErrorData::invalid_params(format!("ask_user retry is missing inputResponses[{key}]"), None))?;
    serde_json::from_value(value)
        .map_err(|error| ErrorData::invalid_params(format!("ask_user retry has a malformed response: {error}"), None))
}

/// Minimal schema check: every `required` property must be present and
/// non-null. Full schema validation is deliberately out of scope here.
fn validate_against_schema(schema: &ElicitationSchema, content: &Value) -> Result<(), String> {
    let object = content.as_object().ok_or_else(|| "response must be an object".to_string())?;
    for required in schema.required.iter().flatten() {
        match object.get(required) {
            Some(value) if !value.is_null() => {}
            _ => return Err(format!("missing required field '{required}'")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schema_from_object_value() {
        let value = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "title": "Name" }
            }
        });
        let schema = parse_schema(value).expect("should parse object value");
        assert!(schema.properties.contains_key("name"));
    }

    #[test]
    fn parse_schema_from_string_value() {
        let json_str = r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#;
        let value = serde_json::Value::String(json_str.to_string());
        let schema = parse_schema(value).expect("should parse string-encoded value");
        assert!(schema.properties.contains_key("name"));
    }

    #[test]
    fn parse_schema_from_empty_object() {
        let value = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        let schema = parse_schema(value).expect("should parse empty schema");
        assert!(schema.properties.is_empty());
    }

    #[test]
    fn parse_schema_from_empty_string_encoded_object() {
        let json_str = r#"{"type":"object","properties":{}}"#;
        let value = serde_json::Value::String(json_str.to_string());
        let schema = parse_schema(value).expect("should parse string-encoded empty schema");
        assert!(schema.properties.is_empty());
    }

    #[test]
    fn parse_schema_rejects_invalid_string() {
        let value = serde_json::Value::String("not json".to_string());
        assert!(parse_schema(value).is_err());
    }

    #[test]
    fn parse_schema_rejects_non_object_type() {
        let value = serde_json::json!({ "type": "array" });
        assert!(parse_schema(value).is_err());
    }

    #[test]
    fn validate_against_schema_accepts_complete_content() {
        let schema = ElicitationSchema::builder().required_string("name").build().unwrap();
        let content = serde_json::json!({ "name": "Ada" });
        assert!(validate_against_schema(&schema, &content).is_ok());
    }

    #[test]
    fn validate_against_schema_rejects_missing_required() {
        let schema = ElicitationSchema::builder().required_string("name").build().unwrap();
        let content = serde_json::json!({ "other": "x" });
        assert!(validate_against_schema(&schema, &content).is_err());
    }

    #[test]
    fn validate_against_schema_rejects_null_required() {
        let schema = ElicitationSchema::builder().required_string("name").build().unwrap();
        let content = serde_json::json!({ "name": null });
        assert!(validate_against_schema(&schema, &content).is_err());
    }
}
