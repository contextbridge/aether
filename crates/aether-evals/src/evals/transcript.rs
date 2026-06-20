use crate::agents::{TRANSCRIPT_PAYLOAD_CHARS, get_transcript_line};
use aether_core::events::{AgentMessage, ContextUsage};
use std::fmt::Write as _;

pub struct Transcript {
    messages: Vec<AgentMessage>,
}

pub struct ToolCall<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

impl Transcript {
    pub(crate) fn new(messages: Vec<AgentMessage>) -> Self {
        Self { messages }
    }

    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    pub fn all_tool_calls(&self) -> impl Iterator<Item = ToolCall<'_>> + '_ {
        self.messages.iter().filter_map(|message| match message {
            AgentMessage::ToolResult { result, .. } => {
                Some(ToolCall { name: &result.name, arguments: &result.arguments })
            }
            AgentMessage::ToolError { error, .. } => {
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
                AgentMessage::ContextUsageUpdate { usage } => Some(usage.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }
}

impl ToolCall<'_> {
    pub fn arguments_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.arguments)
    }
}

pub(crate) fn format_transcript(messages: &[AgentMessage]) -> String {
    let mut transcript = String::new();
    for message in messages {
        if let Some(line) = get_transcript_line(message, TRANSCRIPT_PAYLOAD_CHARS) {
            let _ = writeln!(transcript, "{line}");
        }
    }
    transcript
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{ToolCallRequest, ToolCallResult};

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
    ) -> AgentMessage {
        AgentMessage::ContextUsageUpdate {
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
        }
    }

    pub(crate) fn transcript_with_messages(messages: Vec<AgentMessage>) -> Transcript {
        Transcript::new(messages)
    }

    fn tool_call(name: &str) -> AgentMessage {
        AgentMessage::ToolCall {
            request: ToolCallRequest { id: name.to_string(), name: name.to_string(), arguments: "{}".to_string() },
            model_name: "test".to_string(),
        }
    }

    fn tool_result(name: &str) -> AgentMessage {
        AgentMessage::ToolResult {
            result: ToolCallResult {
                id: name.to_string(),
                name: name.to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        }
    }
}
