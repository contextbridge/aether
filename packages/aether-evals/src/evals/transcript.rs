use crate::agents::{TRANSCRIPT_PAYLOAD_CHARS, get_transcript_line};
use aether_core::events::AgentMessage;
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
