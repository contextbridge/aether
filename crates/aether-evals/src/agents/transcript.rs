use super::{AgentRunResult, RunError};
use crate::EvalRunError;
use aether_core::events::{AgentEvent, ToolEvent};
use futures::{Stream, StreamExt};
use llm::SessionUsageTotals;
use std::fmt::Debug;
use thiserror::Error;

pub struct Transcript {
    events: Vec<AgentEvent>,
}

pub struct ToolCall<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

#[derive(Error)]
#[error("{error}")]
pub struct TranscriptError {
    transcript: Transcript,
    #[source]
    error: EvalRunError,
}

impl Transcript {
    pub fn new(events: Vec<AgentEvent>) -> Self {
        Self { events }
    }

    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {
        let mut transcript = Self::default();
        futures::pin_mut!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    transcript.add(event);
                }
                Err(error) => return Err(TranscriptError::new(transcript, error)),
            }
        }
        Ok(transcript)
    }

    pub fn add(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn all_tool_calls(&self) -> impl Iterator<Item = ToolCall<'_>> + '_ {
        self.events.iter().filter_map(|event| match event {
            AgentEvent::Tool(ToolEvent::Result { result, .. }) => {
                Some(ToolCall { name: &result.name, arguments: &result.arguments })
            }
            AgentEvent::Tool(ToolEvent::Error { error, .. }) => {
                Some(ToolCall { name: &error.name, arguments: error.arguments.as_deref().unwrap_or("") })
            }
            _ => None,
        })
    }

    pub fn tool_calls<'a>(&'a self, name: &'a str) -> impl Iterator<Item = ToolCall<'a>> + 'a {
        self.all_tool_calls().filter(move |call| call.name == name)
    }

    pub fn tool_called(&self, name: &str) -> bool {
        self.tool_calls(name).next().is_some()
    }

    pub fn tool_call_count(&self, name: &str) -> usize {
        self.tool_calls(name).count()
    }

    /// Session-wide token totals and estimated cost from the last usage event,
    /// or zeroed totals if no usage was recorded.
    pub fn usage(&self) -> SessionUsageTotals {
        self.events
            .iter()
            .rev()
            .find_map(|event| match event {
                AgentEvent::SessionUsage(usage) => Some(usage.totals.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl From<Vec<AgentEvent>> for Transcript {
    fn from(events: Vec<AgentEvent>) -> Self {
        Self::new(events)
    }
}

impl ToolCall<'_> {
    pub fn arguments_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.arguments)
    }
}

impl TranscriptError {
    fn new(transcript: Transcript, error: RunError) -> Self {
        Self { transcript, error: EvalRunError::from(error) }
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn error(&self) -> &EvalRunError {
        &self.error
    }

    pub fn into_parts(self) -> (Transcript, EvalRunError) {
        (self.transcript, self.error)
    }
}

impl Debug for TranscriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TranscriptError").field("error", &self.error).finish_non_exhaustive()
    }
}

pub(crate) fn is_terminal(event: &AgentEvent) -> bool {
    event.turn_outcome().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agent, FakeAgent, Task};
    use aether_core::events::TurnEvent;
    use llm::testing::session_usage_event;
    use llm::{TokenUsage, ToolCallRequest, ToolCallResult};

    #[tokio::test]
    async fn transcript_from_stream() {
        let agent = FakeAgent::with_tool_call("bash", "success");
        let stream = agent.run(Task::new("do the thing"));
        let transcript = Transcript::from_stream(stream).await.unwrap();

        assert!(transcript.tool_called("bash"));
        assert!(matches!(transcript.events().last(), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))));
    }

    #[test]
    fn tool_call_count_counts_matching_tool_calls() {
        let transcript = transcript_with_events(vec![tool_call("bash"), tool_call("read"), tool_result("bash")]);

        assert!(transcript.tool_called("bash"));
        assert!(!transcript.tool_called("read"));
        assert!(!transcript.tool_called("write"));
        assert_eq!(transcript.tool_call_count("bash"), 1);
        assert_eq!(transcript.tool_call_count("read"), 0);
    }

    #[test]
    fn tool_call_arguments_json_parses_arguments() {
        let call = ToolCall { name: "bash", arguments: r#"{"command":"pwd"}"# };

        assert_eq!(call.arguments_json().unwrap(), serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    fn tool_call_arguments_json_returns_error_for_invalid_json() {
        let call = ToolCall { name: "bash", arguments: "not json" };

        assert!(call.arguments_json().is_err());
    }

    #[test]
    fn usage_returns_zeroed_totals_when_no_usage_was_recorded() {
        let transcript = transcript_with_events(vec![tool_call("bash")]);
        assert_eq!(transcript.usage(), SessionUsageTotals::default());
    }

    #[test]
    fn usage_extracts_the_final_session_totals() {
        let mut last = session_usage_event(2, TokenUsage::new(2000, 500));
        last.totals.tokens = TokenUsage::new(3000, 600);
        last.totals.unpriced_calls = 2;
        let transcript = transcript_with_events(vec![
            AgentEvent::SessionUsage(session_usage_event(1, TokenUsage::new(1000, 100))),
            AgentEvent::SessionUsage(last),
        ]);

        let usage = transcript.usage();
        assert_eq!(usage.tokens.input_tokens.get(), 3000);
        assert_eq!(usage.tokens.output_tokens.get(), 600);
        assert_eq!(usage.tokens.total_tokens().get(), 3600);
        assert_eq!(usage.unpriced_calls, 2);
        assert!(!usage.is_fully_priced());
    }

    fn transcript_with_events(events: Vec<AgentEvent>) -> Transcript {
        Transcript::new(events)
    }

    fn tool_call(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: ToolCallRequest { id: name.to_string(), name: name.to_string(), arguments: "{}".to_string() },
        })
    }

    fn tool_result(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: name.to_string(),
                name: name.to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        })
    }
}
