use crate::resolve::AgentSelectionError;
use crate::runtime::RuntimeError;
use aether_auth::OAuthError;
use aether_core::mcp::ParseError;
use aether_project::SettingsError;
use aether_telemetry::TelemetryInitError;
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
    #[error(transparent)]
    AgentSelection(#[from] AgentSelectionError),
    #[error("Invalid MCP config: {0}")]
    McpConfig(#[from] ParseError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Agent stopped before receiving the prompt")]
    AgentStopped,
    #[error("Credential store error: {0}")]
    CredentialStore(#[from] OAuthError),
}
