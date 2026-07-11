mod agent;
mod agent_builder;
mod error;
mod prompt;
mod retry_config;

pub use crate::events::{AgentCommand, AgentEvent, Command, UserCommand};
pub use agent::*;
pub use agent_builder::*;
pub use error::*;
pub use prompt::*;
pub use retry_config::RetryConfig;

use llm::StreamingModelProvider;
use std::sync::Arc;

#[doc = include_str!("../docs/basic_agent.md")]
pub fn agent(llm: impl StreamingModelProvider + 'static) -> AgentBuilder {
    AgentBuilder::new(Arc::new(llm))
}
