use crate::events::{AgentEvent, AgentObserver, TraceContext};
use std::sync::{Arc, Mutex};

/// In-memory [`AgentObserver`] that records every event it receives, for
/// asserting on the stream an agent emits.
#[derive(Default)]
pub struct FakeAgentObserver {
    events: Arc<Mutex<Vec<AgentEvent>>>,
    system_prompts: Arc<Mutex<Vec<String>>>,
    panic_on_event: Option<fn(&AgentEvent) -> bool>,
    panic_on_system_prompt: bool,
    panic_on_tool_trace_context: bool,
}

impl FakeAgentObserver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_panic(mut self, predicate: fn(&AgentEvent) -> bool) -> Self {
        self.panic_on_event = Some(predicate);
        self
    }

    pub fn with_system_prompt_panic(mut self) -> Self {
        self.panic_on_system_prompt = true;
        self
    }

    pub fn with_tool_trace_context_panic(mut self) -> Self {
        self.panic_on_tool_trace_context = true;
        self
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
        assert!(!self.panic_on_event.is_some_and(|predicate| predicate(message)), "observer exploded");
    }

    fn on_system_prompt(&mut self, prompt: &str) {
        self.system_prompts.lock().unwrap().push(prompt.to_string());
        assert!(!self.panic_on_system_prompt, "system prompt observer exploded");
    }

    fn tool_trace_context(&self, _tool_id: &str) -> Option<TraceContext> {
        assert!(!self.panic_on_tool_trace_context, "tool trace observer exploded");
        None
    }
}
