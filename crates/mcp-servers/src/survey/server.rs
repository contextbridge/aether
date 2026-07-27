use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        ElicitRequestParams, ElicitationAction, ElicitationSchema, Implementation, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, from_str, from_value};

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
}

impl Default for SurveyMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl SurveyMcp {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    pub fn from_args(_args: Vec<String>) -> Result<Self, crate::error::ServerInitError> {
        Ok(Self::new())
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
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = true
    ))]
    pub async fn ask_user(
        &self,
        request: Parameters<AskUserInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<AskUserOutput>, String> {
        let Parameters(args) = request;
        let schema = parse_schema(args.schema).map_err(|e| e.to_string())?;
        let result = context
            .peer
            .create_elicitation(ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: args.message,
                requested_schema: schema,
            })
            .await
            .map_err(|e| format!("Elicitation failed: {e}"))?;

        Ok(Json(AskUserOutput { accepted: result.action == ElicitationAction::Accept, data: result.content }))
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
}
