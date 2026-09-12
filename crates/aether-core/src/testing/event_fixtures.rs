use crate::events::{AgentEvent, LlmCallOutcome, ToolEvent, TurnEvent};
use llm::{LlmCallPurpose, ModelIdentity, ToolCallError, ToolCallRequest, ToolCallResult};

/// A request for the named tool, as the agent would emit it.
pub fn tool_request(id: &str, name: &str, arguments: &str) -> ToolCallRequest {
    ToolCallRequest { id: id.into(), name: name.into(), arguments: arguments.into() }
}

/// The LLM requested a tool call.
pub fn tool_call(id: &str, name: &str, arguments: &str) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::Call { request: tool_request(id, name, arguments) })
}

/// A chunk of streamed tool-call arguments.
pub fn tool_call_update(id: &str, chunk: &str) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: id.into(), chunk: chunk.into() })
}

/// A completed tool result.
pub fn tool_call_result(id: &str, name: &str, arguments: &str, result: &str) -> ToolCallResult {
    ToolCallResult { id: id.into(), name: name.into(), arguments: arguments.into(), result: result.into() }
}

/// The tool completed successfully, without display metadata.
pub fn tool_result(id: &str, name: &str, arguments: &str, result: &str) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::Result { result: tool_call_result(id, name, arguments, result), result_meta: None })
}

/// The tool failed.
pub fn tool_error(id: &str, name: &str, error: &str) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::Error {
        error: ToolCallError { id: id.into(), name: name.into(), arguments: None, error: error.into() },
    })
}

/// Progress reported by an executing tool.
pub fn tool_progress(id: &str, name: &str, progress: f64, total: Option<f64>, message: Option<&str>) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::Progress {
        request: tool_request(id, name, "{}"),
        progress,
        total,
        message: message.map(Into::into),
    })
}

/// A retry of a failed chat call is waiting `delay_ms` before its request starts.
pub fn retry_scheduled(attempt: u32, max_attempts: u32, delay_ms: u64) -> AgentEvent {
    AgentEvent::Turn(TurnEvent::RetryScheduled { purpose: LlmCallPurpose::Chat, attempt, max_attempts, delay_ms })
}

/// A chat request was issued to the default model with `attempt` (0 for the initial call).
pub fn llm_call_started(attempt: u32) -> AgentEvent {
    AgentEvent::Turn(TurnEvent::LlmCallStarted {
        purpose: LlmCallPurpose::Chat,
        model: ModelIdentity::default(),
        display_name: "test".into(),
        attempt,
        max_attempts: 3,
    })
}

/// A chat call reached a terminal state.
pub fn llm_call_ended(outcome: LlmCallOutcome) -> AgentEvent {
    AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Chat, outcome })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RetryInfo;

    #[test]
    fn tool_fixtures_carry_identity() {
        assert_eq!(tool_call("c1", "bash", "{}").content(), None);
        let AgentEvent::Tool(ToolEvent::Call { request }) = tool_call("c1", "bash", r#"{"cmd":"ls"}"#) else {
            panic!("expected tool call");
        };
        assert_eq!(request.id, "c1");
        assert_eq!(request.name, "bash");
        assert_eq!(request.arguments, r#"{"cmd":"ls"}"#);

        let AgentEvent::Tool(ToolEvent::Result { result, result_meta }) = tool_result("c2", "bash", "{}", "ok") else {
            panic!("expected tool result");
        };
        assert_eq!(result.name, "bash");
        assert_eq!(result.result, "ok");
        assert!(result_meta.is_none());

        let AgentEvent::Tool(ToolEvent::Error { error }) = tool_error("c3", "bash", "boom") else {
            panic!("expected tool error");
        };
        assert_eq!(error.error, "boom");
    }

    #[test]
    fn turn_fixtures_expose_retry_info() {
        let AgentEvent::Turn(turn) = retry_scheduled(1, 3, 10) else { panic!("expected turn event") };
        assert_eq!(turn.retry_info(), Some(RetryInfo { attempt: 1, max_attempts: 3, delay_ms: 10 }));

        let AgentEvent::Turn(TurnEvent::LlmCallStarted { attempt, max_attempts, .. }) = llm_call_started(2) else {
            panic!("expected llm call started");
        };
        assert_eq!((attempt, max_attempts), (2, 3));
    }
}
