use crate::events::{AgentEvent, AgentObserver};
use std::sync::{Arc, Mutex};

/// In-memory [`AgentObserver`] that records every event it receives, for
/// asserting on the stream an agent emits.
#[derive(Default)]
pub struct FakeAgentObserver {
    events: Arc<Mutex<Vec<AgentEvent>>>,
    system_prompts: Arc<Mutex<Vec<String>>>,
}

impl FakeAgentObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared handle to the recorded events; clones observe future events too.
    pub fn events(&self) -> Arc<Mutex<Vec<AgentEvent>>> {
        Arc::clone(&self.events)
    }

    /// Shared handle to the system prompts reported for each LLM request.
    pub fn system_prompts(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.system_prompts)
    }
}

impl AgentObserver for FakeAgentObserver {
    fn on_event(&mut self, message: &AgentEvent) {
        self.events.lock().unwrap().push(message.clone());
    }

    fn on_system_prompt(&mut self, prompt: &str) {
        self.system_prompts.lock().unwrap().push(prompt.to_string());
    }
}
