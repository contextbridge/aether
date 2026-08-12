//! Shared types for agent events.
//!
//! This module provides types used across multiple Aether packages:
//! - Agent event types (`AgentEvent`, `Command`)
//! - ACP protocol extension payloads (`SubAgentProgressPayload`)

mod acp;
mod agent_event;
mod context_event;
mod message_event;
mod model_event;
mod observer;
mod sub_agent_progress;
mod tool_event;
mod trace_context;
mod turn_event;
mod user_message;

pub use acp::{aether_tool_name_meta, humanize_tool_name, mcp_tool_name, parse_tool_call_chunk};
pub use agent_event::AgentEvent;
pub use context_event::{CompactionOutcome, ContextEvent, ContextUsage};
pub use message_event::{MessageEvent, StreamState};
pub use model_event::ModelEvent;
pub use observer::{AgentObserver, DynObserverFactory, McpRequestInstrumentation, ObserverFactory};
pub use sub_agent_progress::SubAgentProgressPayload;
pub use tool_event::{TaskOutcome, TaskOutcomeState, ToolEvent, task_created_result};
pub use trace_context::{TRACEPARENT_KEY, TRACESTATE_KEY, TraceContext};
pub use turn_event::{LlmCallOutcome, LlmCallPurpose, RetryInfo, TurnEvent, TurnOutcome};
pub use user_message::{AgentCommand, Command, UserCommand};
