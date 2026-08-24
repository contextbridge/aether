#![doc = include_str!("../README.md")]

/// Meta key under which Aether agents publish the machine-readable tool name on an ACP
/// `ToolCall._meta`, so consumers can recover the original tool name from the humanized title.
pub const AETHER_TOOL_NAME_META_KEY: &str = "aetherToolName";

pub mod config_meta;
pub mod config_option_id;
pub mod content;
pub mod elicitation;
pub mod notifications;
pub mod settings;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "testing")]
pub mod testing;
