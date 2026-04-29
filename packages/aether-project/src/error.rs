//! Error types for settings loading and validation.

use aether_core::core::PromptSourceError;
use thiserror::Error;

/// Errors that can occur during settings loading and agent resolution.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// The settings file exists but could not be parsed.
    #[error("Failed to parse settings file: {0}")]
    ParseError(String),

    /// An agent entry has an invalid model string.
    #[error("Invalid model '{model}' for agent '{agent}': {error}")]
    InvalidModel { agent: String, model: String, error: String },

    /// An agent entry is missing required fields.
    #[error("Agent '{agent}' is missing required field: {field}")]
    MissingField { agent: String, field: String },

    /// An agent entry has an empty name.
    #[error("Agent at index {index} has an empty name")]
    EmptyAgentName { index: usize },

    /// An agent entry uses a reserved name.
    #[error("Agent name '{name}' is reserved and cannot be used")]
    ReservedAgentName { name: String },

    /// Duplicate agent names.
    #[error("Duplicate agent name: '{name}'")]
    DuplicateAgentName { name: String },

    /// An agent has no invocation surface enabled.
    #[error("Agent '{agent}' must have at least one invocation flag (userInvocable or agentInvocable)")]
    NoInvocationSurface { agent: String },

    /// A prompt source on a specific agent failed validation.
    #[error("Agent '{agent}': {source}")]
    AgentPromptSource {
        agent: String,
        #[source]
        source: PromptSourceError,
    },

    /// A prompt source failed validation outside an agent context.
    #[error(transparent)]
    PromptSource(#[from] PromptSourceError),

    /// An agent has no prompts after inheritance.
    #[error("Agent '{agent}' has no prompts after inheritance (neither inherited nor local)")]
    NoPrompts { agent: String },

    /// An MCP config path does not exist or is invalid.
    #[error("MCP config path '{path}' does not exist or is not a file")]
    InvalidMcpConfigPath { path: String },

    /// I/O error while reading files.
    #[error("I/O error: {0}")]
    IoError(String),

    /// An agent was not found in the catalog.
    #[error("Agent '{name}' not found")]
    AgentNotFound { name: String },

    /// The authored config contains no agents.
    #[error("Aether config must contain at least one agent")]
    EmptyAgents,

    /// The configured agent selector did not match an agent.
    #[error("Configured agent selector '{name}' did not match any agent")]
    InvalidAgentSelector { name: String },

    /// The configured agent selector matched an agent that users cannot invoke.
    #[error("Configured agent selector '{name}' is not user-invocable")]
    NonUserInvocableAgentSelector { name: String },

    /// Duplicate prompt names in the catalog.
    #[error("Duplicate prompt name: '{name}'")]
    DuplicatePromptName { name: String },
}
