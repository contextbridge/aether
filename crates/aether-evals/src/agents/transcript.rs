use super::{AgentRunResult, RunError};
use crate::EvalRunError;
use aether_core::events::{AgentEvent, ContextEvent, ContextUsage, ToolEvent, TurnEvent};
use futures::{Stream, StreamExt};
use std::fmt::Debug;
use thiserror::Error;

pub struct Transcript {
    messages: Vec<AgentEvent>,
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
    pub fn new(messages: Vec<AgentEvent>) -> Self {
        Self { messages }
    }

    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {
        let mut transcript = Self::default();
        futures::pin_mut!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => {
                    transcript.add(message);
                }
                Err(error) => return Err(TranscriptError::new(transcript, error)),
            }
        }
        Ok(transcript)
    }

    pub fn add(&mut self, message: AgentEvent) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[AgentEvent] {
        &self.messages
    }

    pub fn all_tool_calls(&self) -> impl Iterator<Item = ToolCall<'_>> + '_ {
        self.messages.iter().filter_map(|message| match message {
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

    /// Returns the aggregated usage from the final `ContextUsageUpdate`, or a
    /// zeroed summary if no usage was recorded.
    pub fn usage(&self) -> ContextUsage {
        self.messages
            .iter()
            .rev()
            .find_map(|msg| match msg {
                AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(usage.clone()),
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
    fn from(messages: Vec<AgentEvent>) -> Self {
        Self::new(messages)
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

pub(crate) fn is_terminal(message: &AgentEvent) -> bool {
    matches!(message, AgentEvent::Turn(TurnEvent::Ended { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agent, FakeAgent, Task};
    use llm::{ToolCallRequest, ToolCallResult};

    #[tokio::test]
    async fn transcript_from_stream() {
        let agent = FakeAgent::with_tool_call("bash", "success");
        let stream = agent.run(Task::new("do the thing"));
        let transcript = Transcript::from_stream(stream).await.unwrap();

        assert!(transcript.tool_called("bash"));
        assert!(matches!(transcript.messages().last(), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))));
    }

    #[test]
    fn tool_call_count_counts_matching_tool_calls() {
        let transcript = transcript_with_messages(vec![tool_call("bash"), tool_call("read"), tool_result("bash")]);

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
    fn usage_returns_zeroed_summary_when_no_context_usage() {
        let transcript = transcript_with_messages(vec![tool_call("bash")]);
        let usage = transcript.usage();
        assert_eq!(usage, ContextUsage::default());
    }

    #[test]
    fn usage_extracts_final_cumulative_totals() {
        let transcript = transcript_with_messages(vec![
            context_usage(100, 10, 1000, 100, 0, 0),
            context_usage(200, 20, 3000, 600, 50, 0),
        ]);

        let usage = transcript.usage();
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, Some(50));
        assert_eq!(usage.total_input_tokens, 3000);
        assert_eq!(usage.total_output_tokens, 600);
        assert_eq!(usage.usage_ratio, Some(0.5));
        assert_eq!(usage.context_limit, Some(200_000));
    }

    #[test]
    fn total_tokens_sums_input_and_output() {
        let transcript = transcript_with_messages(vec![context_usage(0, 0, 3000, 600, 0, 0)]);

        assert_eq!(transcript.usage().total_tokens(), 3600);
    }

    fn context_usage(
        input_tokens: u32,
        output_tokens: u32,
        total_input: u64,
        total_output: u64,
        cache_read: u32,
        reasoning: u32,
    ) -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage {
                usage_ratio: Some(0.5),
                context_limit: Some(200_000),
                input_tokens,
                output_tokens,
                cache_read_tokens: Some(cache_read),
                reasoning_tokens: Some(reasoning),
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                ..Default::default()
            },
        })
    }

    fn transcript_with_messages(messages: Vec<AgentEvent>) -> Transcript {
        Transcript::new(messages)
    }

    fn tool_call(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: ToolCallRequest { id: name.to_string(), name: name.to_string(), arguments: "{}".to_string() },
            model_name: "test".to_string(),
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
            model_name: "test".to_string(),
        })
    }
}
