//! Shared types for agent events.
//!
//! This module provides types used across multiple Aether packages:
//! - Agent message types (`AgentMessage`, `Command`)
//! - ACP protocol extension payloads (`SubAgentProgressPayload`)

mod acp;
mod agent_message;
mod sub_agent_progress;
mod user_message;

pub use acp::{
    AcpAgentMessageMapper, aether_tool_name_meta, humanize_tool_name, parse_tool_call_chunk, tool_name_from_meta,
};
pub use agent_message::AgentMessage;
pub use sub_agent_progress::SubAgentProgressPayload;
pub use user_message::{AgentCommand, Command, UserCommand};
