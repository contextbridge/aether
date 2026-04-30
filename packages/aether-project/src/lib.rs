#![doc = include_str!("../README.md")]

pub mod aether_settings;
mod agent_catalog;
mod agent_config;
mod error;
mod mcp_config_source_config;
mod prompt_catalog;
pub mod prompt_file;

pub use aether_core::core::{PromptSource, PromptSourceError};
pub use aether_settings::{AetherSettings, AetherSettingsSource, SettingsFileSource};
pub use agent_catalog::AgentCatalog;
pub use agent_config::AgentConfig;
pub use error::SettingsError;
pub use mcp_config_source_config::McpSourceSpec;
pub use prompt_catalog::PromptCatalog;
pub use prompt_file::{PromptFile, PromptFileError, PromptTriggers, SKILL_FILENAME};
