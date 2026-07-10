use crate::events::AgentEvent;
use std::sync::Arc;

/// Observer of the agent's event stream. Observers see every message the
/// agent emits, including internal trace events that UI and persistence consumers skip.
pub trait AgentObserver: Send {
    fn on_event(&mut self, message: &AgentEvent);
}

pub type ObserverFactory = Arc<dyn Fn() -> Box<dyn AgentObserver> + Send + Sync>;
