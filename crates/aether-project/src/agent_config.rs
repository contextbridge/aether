use crate::{McpSourceSpec, PromptSource};
use aether_core::agent_spec::ToolFilter;
use llm::{ModelSettings, ProviderConnectionOverrides, ReasoningEffort};

#[doc = include_str!("docs/agent_config.md")]
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(transform = require_agent_invocation_surface_schema)]
pub struct AgentConfig {
    /// Agent identifier. Names are trimmed for merge and lookup.
    #[schemars(length(min = 1))]
    pub name: String,
    /// Human-readable description shown in UIs and sub-agent listings.
    #[schemars(length(min = 1))]
    pub description: String,
    /// Model spec for this agent, in `provider:model-id` form. Accepts a
    /// comma-separated alloy of specs to round-robin across turns.
    #[schemars(length(min = 1))]
    pub model: String,
    /// Optional thinking budget for providers that support extended reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Sampling controls (`temperature`, `topP`, `maxTokens`) applied to this
    /// agent's model calls. Omitted knobs keep the provider/model default.
    #[serde(default, skip_serializing_if = "ModelSettings::is_empty")]
    pub model_settings: ModelSettings,
    /// Override for the model's context window, in tokens. Defaults to the
    /// model's advertised window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub context_window: Option<u32>,
    /// Exposes the agent as a user-selectable mode.
    #[serde(default)]
    pub user_invocable: bool,
    /// Exposes the agent to the `subagents` MCP server so other agents can spawn it.
    #[serde(default)]
    pub agent_invocable: bool,
    /// Per-agent provider overrides, merged over the top-level `providers`.
    #[serde(default, skip_serializing_if = "ProviderConnectionOverrides::is_empty")]
    pub providers: ProviderConnectionOverrides,
    /// Agent-specific prompt sources. A non-empty array replaces the top-level `prompts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(min = 1))]
    pub prompts: Vec<PromptSource>,
    /// Agent-specific MCP sources. A non-empty array replaces the top-level `mcps`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<McpSourceSpec>,
    /// Per-agent MCP tool filter (allow/deny lists).
    #[serde(default, skip_serializing_if = "ToolFilter::is_empty")]
    pub tools: ToolFilter,
}

fn require_agent_invocation_surface_schema(schema: &mut schemars::Schema) {
    let Some(mut base_schema) = schema.as_object().cloned() else {
        return;
    };
    let description = base_schema.remove("description");

    let invocation_surface_schema = serde_json::json!({
        "anyOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["userInvocable"],
                "properties": { "userInvocable": { "const": true } }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["agentInvocable"],
                "properties": { "agentInvocable": { "const": true } }
            }
        ]
    });

    let mut composed_schema = serde_json::Map::new();
    if let Some(description) = description {
        composed_schema.insert("description".to_string(), description);
    }
    composed_schema.insert("allOf".to_string(), serde_json::json!([base_schema, invocation_surface_schema]));
    *schema = composed_schema.into();
}
