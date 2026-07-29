use crate::events::{AgentEvent, TraceContext};
use std::sync::Arc;

/// Observer of the agent's event stream. Observers receive the same events
/// the agent sends on its event channel, synchronously and in order —
/// including events that channel consumers such as UIs and session
/// persistence filter out downstream.
pub trait AgentObserver: Send {
    fn on_event(&mut self, message: &AgentEvent);

    /// Trace context to propagate to whatever executes `tool_id`, available once
    /// the observer has seen that tool's
    /// [`ExecutionStarted`](crate::events::ToolEvent::ExecutionStarted).
    fn tool_trace_context(&self, _tool_id: &str) -> Option<TraceContext> {
        None
    }
}

/// Instrumentation for one inbound MCP request, held by the server handling it.
pub trait McpRequestInstrumentation: Send {
    /// Context beneath the request, for application work the request spawns.
    fn trace_context(&self) -> Option<TraceContext>;

    /// Completes the request. `error` is the public operation error, if any.
    fn finish(self: Box<Self>, error: Option<&str>);
}

/// Creates observers isolated to one agent or one inbound MCP request.
pub trait ObserverFactory: Send + Sync {
    /// A fresh observer for one agent, continuing the parent trace when supplied.
    fn agent(&self, agent_name: Option<&str>, parent: Option<&TraceContext>) -> Box<dyn AgentObserver>;

    /// Instrumentation for one inbound `tools/call` request.
    fn tool_call_request(&self, tool_name: &str, parent: Option<&TraceContext>) -> Box<dyn McpRequestInstrumentation>;
}

pub type DynObserverFactory = Arc<dyn ObserverFactory>;
