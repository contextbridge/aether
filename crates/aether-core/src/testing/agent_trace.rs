use crate::events::{ToolEvent, TurnEvent};
use std::sync::{Arc, Mutex};

use crate::events::{AgentEvent, LlmCallOutcome, TurnOutcome};
use llm::LlmCallPurpose;

/// A recorded agent event stream (as seen by an
/// [`AgentObserver`](crate::events::AgentObserver)), with assertion helpers
/// for trace-shaped tests. Build one with
/// [`TestAgentBuilder::run_trace`](crate::testing::TestAgentBuilder::run_trace).
pub struct AgentTrace {
    events: Vec<AgentEvent>,
}

impl AgentTrace {
    pub fn from_events(events: Vec<AgentEvent>) -> Self {
        Self { events }
    }

    pub fn from_observer_events(events: &Arc<Mutex<Vec<AgentEvent>>>) -> Self {
        Self::from_events(events.lock().unwrap().clone())
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn assert_names(&self, expected: &[&str]) {
        assert_eq!(map_event_names(&self.events), expected, "unexpected trace: {:?}", self.events);
    }

    /// Index into [`Self::events`] of the first event matching `predicate`.
    pub fn position(&self, predicate: impl Fn(&AgentEvent) -> bool) -> usize {
        self.events
            .iter()
            .position(predicate)
            .unwrap_or_else(|| panic!("expected event not found in trace: {:?}", self.events))
    }

    /// Indexes into [`Self::events`] of every event matching `predicate`.
    pub fn positions(&self, predicate: impl Fn(&AgentEvent) -> bool) -> Vec<usize> {
        self.events.iter().enumerate().filter(|(_, event)| predicate(event)).map(|(index, _)| index).collect()
    }

    pub fn call_usage(&self, for_purpose: LlmCallPurpose) -> Option<llm::TokenUsage> {
        self.events.iter().find_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose, outcome: LlmCallOutcome::Completed { usage, .. } })
                if *purpose == for_purpose =>
            {
                *usage
            }
            _ => None,
        })
    }
}

/// Maps trace-relevant events to compact names for order assertions,
/// skipping content messages.
pub fn map_event_names(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(TurnEvent::Started { .. }) => Some("turn_started".to_string()),
            AgentEvent::Turn(TurnEvent::Ended { outcome }) => {
                let outcome = match outcome {
                    TurnOutcome::Completed => "completed",
                    TurnOutcome::Cancelled => "cancelled",
                    TurnOutcome::Failed { .. } => "failed",
                };
                Some(format!("turn_ended:{outcome}"))
            }
            AgentEvent::Turn(TurnEvent::RetryScheduled { purpose, attempt, .. }) => {
                Some(format!("retry_scheduled:{purpose:?}:{attempt}"))
            }
            AgentEvent::Turn(TurnEvent::LlmCallStarted { purpose, attempt, .. }) => {
                Some(format!("call_started:{purpose:?}:{attempt}"))
            }
            AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose, outcome }) => {
                let outcome = match outcome {
                    LlmCallOutcome::Completed { .. } => "completed",
                    LlmCallOutcome::Failed { will_retry: true, .. } => "failed_will_retry",
                    LlmCallOutcome::Failed { will_retry: false, .. } => "failed_terminal",
                    LlmCallOutcome::Cancelled => "cancelled",
                };
                Some(format!("call_ended:{purpose:?}:{outcome}"))
            }
            AgentEvent::Tool(ToolEvent::ExecutionStarted { .. }) => Some("tool_execution_started".to_string()),
            AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. }) => Some("tool_definitions".to_string()),
            _ => None,
        })
        .collect()
}
