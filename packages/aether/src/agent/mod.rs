mod agent_builder;
mod core;
mod error;
mod messages;
pub mod middleware;
mod prompt;
mod substitution;

pub use agent_builder::*;
pub use core::*;
pub use error::*;
pub use messages::*;
pub use middleware::{AgentEvent, Middleware, MiddlewareAction};
pub use prompt::*;
pub use substitution::*;

use crate::llm::StreamingModelProvider;

pub fn agent<T: StreamingModelProvider + 'static>(llm: T) -> AgentBuilder<T> {
    AgentBuilder::new(llm)
}
