use crate::{McpSourceSpec, PromptSource};
use aether_core::agent_spec::ToolFilter;
use llm::ReasoningEffort;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(transform = require_agent_invocation_surface_schema)]
pub struct AgentConfig {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub description: String,
    #[schemars(length(min = 1))]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub user_invocable: bool,
    #[serde(default)]
    pub agent_invocable: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(min = 1))]
    pub prompts: Vec<PromptSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<McpSourceSpec>,
    #[serde(default, skip_serializing_if = "ToolFilter::is_empty")]
    pub tools: ToolFilter,
}

fn require_agent_invocation_surface_schema(schema: &mut schemars::Schema) {
    schema.insert(
        "anyOf".to_string(),
        serde_json::json!([
            { "required": ["userInvocable"], "properties": { "userInvocable": { "const": true } } },
            { "required": ["agentInvocable"], "properties": { "agentInvocable": { "const": true } } }
        ]),
    );
}
