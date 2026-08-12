use crate::clamp_i64;
use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome};
use aether_core::session::{SessionControlEvent, SessionEvent, UserEvent};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventRow {
    pub session_id: String,
    pub event_index: i64,
    pub line_number: i64,
    pub turn_index: Option<i64>,
    pub content: Option<String>,
    pub content_len: i64,
    pub raw_json: String,
    pub kind: &'static str,
    pub event_type: &'static str,
    pub outcome: Option<&'static str>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments: Option<String>,
    pub model_name: Option<String>,
    pub message_id: Option<String>,
    pub usage_ratio: Option<f64>,
    pub context_limit: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub total_cache_read_tokens: Option<i64>,
    pub total_cache_creation_tokens: Option<i64>,
    pub total_reasoning_tokens: Option<i64>,
}

pub(crate) fn event_row(
    session_id: &str,
    event_index: i64,
    line_number: i64,
    turn_index: Option<i64>,
    event: &SessionEvent,
    raw_json: String,
) -> EventRow {
    let content = event.content();
    let content_len = content.as_ref().map_or(0, |content| clamp_i64(content.chars().count()));
    let projection = EventProjection::from(event);

    EventRow {
        session_id: session_id.to_string(),
        event_index,
        line_number,
        turn_index,
        content,
        content_len,
        raw_json,
        kind: projection.kind,
        event_type: projection.event_type,
        outcome: projection.outcome,
        tool_call_id: projection.tool_call_id,
        tool_name: projection.tool_name,
        tool_arguments: projection.tool_arguments,
        model_name: projection.model_name,
        message_id: projection.message_id,
        usage_ratio: projection.usage_ratio,
        context_limit: projection.context_limit,
        input_tokens: projection.input_tokens,
        output_tokens: projection.output_tokens,
        cache_read_tokens: projection.cache_read_tokens,
        cache_creation_tokens: projection.cache_creation_tokens,
        reasoning_tokens: projection.reasoning_tokens,
        total_input_tokens: projection.total_input_tokens,
        total_output_tokens: projection.total_output_tokens,
        total_cache_read_tokens: projection.total_cache_read_tokens,
        total_cache_creation_tokens: projection.total_cache_creation_tokens,
        total_reasoning_tokens: projection.total_reasoning_tokens,
    }
}

#[derive(Default)]
struct EventProjection {
    kind: &'static str,
    event_type: &'static str,
    outcome: Option<&'static str>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_arguments: Option<String>,
    model_name: Option<String>,
    message_id: Option<String>,
    usage_ratio: Option<f64>,
    context_limit: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    total_input_tokens: Option<i64>,
    total_output_tokens: Option<i64>,
    total_cache_read_tokens: Option<i64>,
    total_cache_creation_tokens: Option<i64>,
    total_reasoning_tokens: Option<i64>,
}

impl From<&SessionEvent> for EventProjection {
    fn from(event: &SessionEvent) -> Self {
        match event {
            SessionEvent::User(UserEvent::Message { .. }) => Self::new("user", "user_message"),
            SessionEvent::User(UserEvent::ClearContext) => Self::new("user", "clear_context"),
            SessionEvent::Control(SessionControlEvent::AgentSwitched { .. }) => Self::new("control", "agent_switched"),
            SessionEvent::Agent(event) => Self::from(event),
        }
    }
}

impl From<&AgentEvent> for EventProjection {
    fn from(event: &AgentEvent) -> Self {
        match event {
            AgentEvent::Message(MessageEvent::Text { message_id, .. }) => {
                Self { message_id: Some(message_id.clone()), ..Self::new("agent", "message_text") }
            }
            AgentEvent::Message(MessageEvent::Thought { message_id, .. }) => {
                Self { message_id: Some(message_id.clone()), ..Self::new("agent", "message_thought") }
            }
            AgentEvent::Tool(event) => Self::from_tool(event),
            AgentEvent::Turn(event) => Self::from_turn(event),
            AgentEvent::Context(event) => Self::from_context(event),
            AgentEvent::Model(ModelEvent::Switched { new, .. }) => {
                Self { model_name: Some(new.clone()), ..Self::new("agent", "model_switched") }
            }
        }
    }
}

impl EventProjection {
    fn new(kind: &'static str, event_type: &'static str) -> Self {
        Self { kind, event_type, ..Self::default() }
    }

    fn from_tool(event: &ToolEvent) -> Self {
        match event {
            ToolEvent::Call { request } => Self::for_tool_request("tool_call", request, None),
            ToolEvent::Result { result, .. } => Self {
                tool_call_id: Some(result.id.clone()),
                tool_name: Some(result.name.clone()),
                tool_arguments: Some(result.arguments.clone()),
                ..Self::new("agent", "tool_result")
            },
            ToolEvent::Error { error } => Self {
                tool_call_id: Some(error.id.clone()),
                tool_name: Some(error.name.clone()),
                tool_arguments: error.arguments.clone(),
                outcome: Some("failed"),
                ..Self::new("agent", "tool_error")
            },
            ToolEvent::TaskCreated { request, .. } => Self::for_tool_request("tool_deferred", request, None),
            ToolEvent::TaskStatus { request, .. } => Self::for_tool_request("tool_task_status", request, None),
            ToolEvent::TaskCompleted { request, .. } => {
                Self::for_tool_request("tool_task_completed", request, Some("completed"))
            }
            ToolEvent::TaskFailed { request, .. } => {
                Self::for_tool_request("tool_task_failed", request, Some("failed"))
            }
            ToolEvent::TaskCancelled { request, .. } => {
                Self::for_tool_request("tool_task_cancelled", request, Some("cancelled"))
            }
            ToolEvent::CallUpdate { .. } => Self::new("agent", "tool_call_update"),
            ToolEvent::ExecutionStarted { .. } => Self::new("agent", "tool_execution_started"),
            ToolEvent::Progress { .. } => Self::new("agent", "tool_progress"),
            ToolEvent::DefinitionsUpdated { .. } => Self::new("agent", "tool_definitions_updated"),
        }
    }

    fn from_turn(event: &TurnEvent) -> Self {
        match event {
            TurnEvent::Started { .. } => Self::new("agent", "turn_started"),
            TurnEvent::RetryScheduled { .. } => Self::new("agent", "retry_scheduled"),
            TurnEvent::LlmCallStarted { model, display_name, .. } => Self {
                model_name: model.clone().or_else(|| Some(display_name.clone())),
                ..Self::new("agent", "llm_call_started")
            },
            TurnEvent::LlmCallEnded { outcome, .. } => Self {
                outcome: Some(match outcome {
                    aether_core::events::LlmCallOutcome::Completed { .. } => "completed",
                    aether_core::events::LlmCallOutcome::Failed { .. } => "failed",
                    aether_core::events::LlmCallOutcome::Cancelled => "cancelled",
                }),
                ..Self::new("agent", "llm_call_ended")
            },
            TurnEvent::AutoContinue { .. } => Self::new("agent", "auto_continue"),
            TurnEvent::Ended { outcome } => Self {
                outcome: Some(match outcome {
                    TurnOutcome::Completed => "completed",
                    TurnOutcome::Cancelled => "cancelled",
                    TurnOutcome::Failed { .. } => "failed",
                }),
                ..Self::new("agent", "turn_ended")
            },
        }
    }

    fn from_context(event: &ContextEvent) -> Self {
        match event {
            ContextEvent::CompactionStarted { .. } => Self::new("agent", "context_compaction_started"),
            ContextEvent::CompactionEnded { outcome } => Self {
                outcome: Some(match outcome {
                    aether_core::events::CompactionOutcome::Completed => "completed",
                    aether_core::events::CompactionOutcome::Failed { .. } => "failed",
                    aether_core::events::CompactionOutcome::Cancelled => "cancelled",
                }),
                ..Self::new("agent", "context_compaction_ended")
            },
            ContextEvent::CompactionResult { .. } => Self::new("agent", "context_compaction_result"),
            ContextEvent::UsageUpdated { usage } => Self {
                usage_ratio: usage.usage_ratio,
                context_limit: usage.context_limit.map(clamp_i64),
                input_tokens: Some(clamp_i64(usage.input_tokens)),
                output_tokens: Some(clamp_i64(usage.output_tokens)),
                cache_read_tokens: usage.cache_read_tokens.map(clamp_i64),
                cache_creation_tokens: usage.cache_creation_tokens.map(clamp_i64),
                reasoning_tokens: usage.reasoning_tokens.map(clamp_i64),
                total_input_tokens: Some(clamp_i64(usage.total_input_tokens)),
                total_output_tokens: Some(clamp_i64(usage.total_output_tokens)),
                total_cache_read_tokens: Some(clamp_i64(usage.total_cache_read_tokens)),
                total_cache_creation_tokens: Some(clamp_i64(usage.total_cache_creation_tokens)),
                total_reasoning_tokens: Some(clamp_i64(usage.total_reasoning_tokens)),
                ..Self::new("agent", "context_usage")
            },
            ContextEvent::Cleared => Self::new("agent", "context_cleared"),
        }
    }

    fn for_tool_request(
        event_type: &'static str,
        request: &llm::ToolCallRequest,
        outcome: Option<&'static str>,
    ) -> Self {
        Self {
            tool_call_id: Some(request.id.clone()),
            tool_name: Some(request.name.clone()),
            tool_arguments: Some(request.arguments.clone()),
            outcome,
            ..Self::new("agent", event_type)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::{ContextUsage, LlmCallPurpose};

    #[test]
    fn typed_projection_covers_retry_cancellation_model_and_usage() {
        let retry = SessionEvent::Agent(AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: LlmCallPurpose::Chat,
            attempt: 1,
            max_attempts: 3,
            delay_ms: 10,
        }));
        let cancelled = SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled }));
        let switched = SessionEvent::Agent(AgentEvent::Model(ModelEvent::Switched {
            previous: "old".to_string(),
            new: "new".to_string(),
        }));
        let usage = SessionEvent::Agent(AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage { usage_ratio: Some(0.9), ..ContextUsage::default() },
        }));
        let compaction_ended = SessionEvent::Agent(AgentEvent::Context(ContextEvent::CompactionEnded {
            outcome: aether_core::events::CompactionOutcome::Completed,
        }));

        assert_eq!(EventProjection::from(&retry).event_type, "retry_scheduled");
        assert_eq!(EventProjection::from(&cancelled).outcome, Some("cancelled"));
        assert_eq!(EventProjection::from(&switched).model_name.as_deref(), Some("new"));
        assert_eq!(EventProjection::from(&usage).usage_ratio, Some(0.9));
        assert_eq!(EventProjection::from(&compaction_ended).event_type, "context_compaction_ended");
        assert_eq!(EventProjection::from(&compaction_ended).outcome, Some("completed"));
    }
}
