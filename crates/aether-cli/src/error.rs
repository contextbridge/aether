use aether_auth::OAuthError;
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
    #[error("Model error: {0}")]
    ModelError(String),
    #[error("MCP error: {0}")]
    McpError(String),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Agent error: {0}")]
    AgentError(String),
    #[error("Credential store error: {0}")]
    CredentialStore(#[from] OAuthError),
}
