use crate::events::{AgentObserver, McpRequestInstrumentation, ObserverFactory, TraceContext};
use crate::testing::FakeAgentObserver;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct FakeObserverFactory {
    agents: Arc<Mutex<Vec<ObservedAgent>>>,
    finishes: Arc<Mutex<Vec<Option<String>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAgent {
    pub agent_name: Option<String>,
    pub conversation_id: Option<String>,
}

impl FakeObserverFactory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn agents(&self) -> Arc<Mutex<Vec<ObservedAgent>>> {
        Arc::clone(&self.agents)
    }

    pub fn finishes(&self) -> Arc<Mutex<Vec<Option<String>>>> {
        Arc::clone(&self.finishes)
    }
}

impl ObserverFactory for FakeObserverFactory {
    fn agent(
        &self,
        agent_name: Option<&str>,
        conversation_id: Option<&str>,
        _parent: Option<&TraceContext>,
    ) -> Box<dyn AgentObserver> {
        self.agents.lock().unwrap().push(ObservedAgent {
            agent_name: agent_name.map(str::to_string),
            conversation_id: conversation_id.map(str::to_owned),
        });
        Box::new(FakeAgentObserver::new())
    }

    fn tool_call_request(
        &self,
        _tool_name: &str,
        _parent: Option<&TraceContext>,
    ) -> Box<dyn McpRequestInstrumentation> {
        Box::new(FakeMcpRequestInstrumentation { finishes: Arc::clone(&self.finishes) })
    }
}

struct FakeMcpRequestInstrumentation {
    finishes: Arc<Mutex<Vec<Option<String>>>>,
}

impl McpRequestInstrumentation for FakeMcpRequestInstrumentation {
    fn trace_context(&self) -> Option<TraceContext> {
        None
    }

    fn finish(self: Box<Self>, error: Option<&str>) {
        self.finishes.lock().unwrap().push(error.map(str::to_string));
    }
}
