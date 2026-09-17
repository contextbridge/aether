#![doc = include_str!("../README.md")]

#[cfg(feature = "settings")]
pub mod aether_settings;
#[cfg(feature = "settings")]
mod agent_catalog;
#[cfg(feature = "settings")]
mod agent_config;
mod error;
#[cfg(feature = "settings")]
mod mcp_config_source_config;
mod prompt_catalog;
pub mod prompt_file;
#[cfg(feature = "testing")]
pub mod testing;

#[cfg(feature = "settings")]
pub use aether_core::core::{PromptSource, PromptSourceError};
#[cfg(feature = "settings")]
pub use aether_settings::{
    AetherSettings, AetherSettingsSource, CredentialsStoreConfig, OtlpTelemetrySettings, SettingsFileSource,
    TelemetryContentSettings, TelemetrySettings, project_settings_exist, project_settings_path, settings_resource_root,
    user_settings_exist, user_settings_path,
};
#[cfg(feature = "settings")]
pub use agent_catalog::AgentCatalog;
#[cfg(feature = "settings")]
pub use agent_config::AgentConfig;
pub use error::SettingsError;
#[cfg(feature = "settings")]
pub use mcp_config_source_config::{McpFileSpec, McpSourceSpec};
pub use prompt_catalog::PromptCatalog;
pub use prompt_file::{PromptFile, PromptFileError, PromptTriggers, SKILL_FILENAME};
