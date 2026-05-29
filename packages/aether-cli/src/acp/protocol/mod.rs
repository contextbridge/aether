//! ACP protocol mappers.
//!
//! Each submodule owns one direction of translation between Aether core types
//! and the `agent_client_protocol` schema:

pub(crate) mod commands;
pub(crate) mod content;
pub(crate) mod events;
pub(crate) mod mcp;
pub(crate) mod replay;

pub use commands::map_mcp_prompt_to_available_command;
