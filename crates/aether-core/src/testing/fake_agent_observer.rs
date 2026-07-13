use crate::events::{AgentEvent, AgentObserver};
use std::sync::{Arc, Mutex};

/// In-memory [`AgentObserver`] that records every event it receives, for
/// asserting on the stream an agent emits.
#[derive(Default)]
pub struct FakeAgentObserver {
    events: Arc<Mutex<Vec<AgentEvent>>>,
}

impl FakeAgentObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared handle to the recorded events; clones observe future events too.
    pub fn events(&self) -> Arc<Mutex<Vec<AgentEvent>>> {
        Arc::clone(&self.events)
    }
}

impl AgentObserver for FakeAgentObserver {
    fn on_event(&mut self, message: &AgentEvent) {
        self.events.lock().unwrap().push(message.clone());
    }
}
