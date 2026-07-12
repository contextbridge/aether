use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[doc = include_str!("docs/tools.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    pub title: Option<String>,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self { name: name.into(), description: description.into(), parameters, server: None, annotations: None }
    }

    pub fn with_server(mut self, server: impl Into<String>) -> Self {
        self.server = Some(server.into());
        self
    }

    pub fn with_annotations(mut self, annotations: impl Into<Option<ToolAnnotations>>) -> Self {
        self.annotations = annotations.into();
        self
    }
}

impl ToolAnnotations {
    pub fn read_only() -> Self {
        Self { read_only_hint: Some(true), ..Self::default() }
    }
}

/// Tool call request from the LLM
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Successful result of a tool call
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub result: String,
}

/// Error result of a tool call
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallError {
    pub id: String,
    pub name: String,
    pub arguments: Option<String>,
    pub error: String,
}

impl ToolCallError {
    pub fn from_request(request: &ToolCallRequest, error: impl Into<String>) -> Self {
        Self {
            id: request.id.clone(),
            name: request.name.clone(),
            arguments: Some(request.arguments.clone()),
            error: error.into(),
        }
    }
}
