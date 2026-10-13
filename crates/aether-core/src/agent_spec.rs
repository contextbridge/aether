//! Agent specification types for authored agent definitions.
//!
//! `AgentSpec` is the canonical abstraction for authored agent definitions across the stack.
//! It represents a resolved runtime type, not a raw settings DTO.

use crate::core::Prompt;
use crate::mcp::{McpConfig, ToolFilter};
use llm::{LlmModel, ModelSettings, ProviderConnectionOverrides, ReasoningEffort};

/// A resolved agent specification ready for runtime use.
///
/// This type is produced by validating and resolving authored agent configuration.
/// All validation happens before constructing these runtime types.
#[derive(Debug, Clone, Default)]
pub struct AgentSpec {
    /// The canonical lookup key for this agent.
    pub name: String,
    /// Human-readable description of this agent's purpose.
    pub description: String,
    /// The validated model spec to use for this agent.
    ///
    /// This is stored as a canonical string so authored settings can represent
    /// both single models (`provider:model`) and alloy specs
    /// (`provider1:model1,provider2:model2`).
    pub model: String,
    /// Optional reasoning effort level for models that support it.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Sampling controls applied to this agent's model calls.
    pub model_settings: ModelSettings,
    /// Effective context window in tokens for this agent.
    pub context_window: Option<u32>,
    /// The prompt stack for this agent.
    pub prompts: Vec<Prompt>,
    /// Provider connection overrides keyed by model provider name.
    pub provider_connections: ProviderConnectionOverrides,
    /// MCP servers this agent connects to
    pub mcp: McpConfig,
    /// How this agent can be invoked.
    pub exposure: AgentSpecExposure,
    /// Tool filter for restricting which MCP tools this agent can use.
    pub tools: ToolFilter,
}

impl AgentSpec {
    /// Create a bare no-mode spec without catalog defaults or runtime policy.
    /// Production callers should prefer their catalog's `default_spec` API.
    pub fn bare(model: &LlmModel, reasoning_effort: Option<ReasoningEffort>, prompts: Vec<Prompt>) -> Self {
        Self {
            name: "__default__".to_string(),
            description: "Default agent".to_string(),
            model: model.to_string(),
            reasoning_effort,
            prompts,
            ..Self::default()
        }
    }
}

/// Defines how an agent can be invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AgentSpecExposure {
    /// Whether this agent can be invoked by users (e.g., as an ACP mode).
    pub user_invocable: bool,
    /// Whether this agent can be invoked by other agents (e.g., as a sub-agent).
    pub agent_invocable: bool,
}

impl AgentSpecExposure {
    /// Create an exposure that is only user invocable.
    pub fn user_only() -> Self {
        Self { user_invocable: true, agent_invocable: false }
    }

    /// Create an exposure that is only agent invocable.
    pub fn agent_only() -> Self {
        Self { user_invocable: false, agent_invocable: true }
    }

    /// Create an exposure that is both user and agent invocable.
    pub fn both() -> Self {
        Self { user_invocable: true, agent_invocable: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_spec_has_expected_fields() {
        let model: LlmModel = "anthropic:claude-sonnet-4-5".parse().unwrap();
        let prompts = vec![Prompt::file(PathBuf::from("/tmp/BASE.md"), PathBuf::from("/tmp"))];
        let spec = AgentSpec::bare(&model, None, prompts.clone());

        assert_eq!(spec.name, "__default__");
        assert_eq!(spec.description, "Default agent");
        assert_eq!(spec.model, model.to_string());
        assert!(spec.reasoning_effort.is_none());
        assert_eq!(spec.prompts.len(), 1);
        assert!(spec.mcp.is_empty());
        assert_eq!(spec.exposure, AgentSpecExposure::default());
    }
}
