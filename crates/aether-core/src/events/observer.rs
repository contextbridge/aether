use crate::events::AgentEvent;
use std::sync::Arc;

/// Observer of the agent's event stream. Observers receive the same events
/// the agent sends on its event channel, synchronously and in order —
/// including events that channel consumers such as UIs and session
/// persistence filter out downstream.
pub trait AgentObserver: Send {
    fn on_event(&mut self, message: &AgentEvent);
}

pub type ObserverFactory = Arc<dyn Fn() -> Box<dyn AgentObserver> + Send + Sync>;
