use aether_auth::OAuthError;
use aether_core::mcp::McpSpawnError;
use aether_project::SettingsError;
use aether_telemetry::TelemetryInitError;
use mcp_utils::config::ParseError;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("No prompt provided. Pass a prompt as an argument or pipe via stdin.")]
    NoPrompt,
    #[error("{0}")]
    ConflictingArgs(String),
    #[error("Invalid --options-json: {0}")]
    InvalidOptionsJson(#[source] serde_json::Error),
    #[error(transparent)]
    ConflictingSettingsSources(#[from] crate::settings_args::ConflictingSettingsSources),
    #[error("Failed to load settings: {0}")]
    Settings(#[from] SettingsError),
    #[error("Failed to initialize telemetry: {0}")]
    Telemetry(#[from] TelemetryInitError),
    #[error("Model error: {0}")]
    ModelError(String),
    #[error("Invalid MCP config: {0}")]
    McpConfig(#[from] ParseError),
    #[error("MCP error: {0}")]
    McpSpawn(#[from] McpSpawnError),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Agent error: {0}")]
    AgentError(String),
    #[error("Credential store error: {0}")]
    CredentialStore(#[from] OAuthError),
}
