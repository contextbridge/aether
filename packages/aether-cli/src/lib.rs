#![doc = include_str!("../README.md")]

pub mod acp;
pub mod error;
pub mod headless;
pub mod init;
pub mod mcp_config_args;
pub mod provider_connection_args;
pub mod resolve;
pub mod runtime;
pub mod sandbox;
pub mod settings;
pub mod settings_args;
pub mod show_prompt;

pub use acp::map_mcp_prompt_to_available_command;
