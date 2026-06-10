use aether_core::events::AgentMessage;

pub(crate) const TRANSCRIPT_PAYLOAD_CHARS: usize = 2_000;

pub(crate) fn is_terminal(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::Done | AgentMessage::Error { .. } | AgentMessage::Cancelled { .. })
}

pub(crate) fn get_transcript_line(message: &AgentMessage, max_payload_chars: usize) -> Option<String> {
    match message {
        AgentMessage::Text { chunk, is_complete: true, .. } if !chunk.is_empty() => {
            Some(format!("[agent] {}", truncate_chars(chunk, max_payload_chars)))
        }
        AgentMessage::ToolCall { request, .. } => Some(format!(
            "[tool-call] {} arguments={}",
            request.name,
            truncate_chars(&request.arguments, max_payload_chars)
        )),
        AgentMessage::ToolResult { result, .. } => {
            Some(format!("[tool-result] {}: {}", result.name, truncate_chars(&result.result, max_payload_chars)))
        }
        AgentMessage::ToolError { error, .. } => {
            Some(format!("[tool-error] {}", truncate_chars(&format!("{error:?}"), max_payload_chars)))
        }
        AgentMessage::Error { message } => Some(format!("[error] {}", truncate_chars(message, max_payload_chars))),
        AgentMessage::Cancelled { message } => {
            Some(format!("[error] Cancelled: {}", truncate_chars(message, max_payload_chars)))
        }
        AgentMessage::Done => Some("[done]".to_string()),
        _ => None,
    }
}

pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}... [truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{ToolCallRequest, ToolCallResult};

    #[test]
    fn transcript_lines_label_each_message_kind() {
        let call = AgentMessage::ToolCall {
            request: ToolCallRequest {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            model_name: "test".to_string(),
        };

        assert_eq!(get_transcript_line(&AgentMessage::text("msg_1", "hi", true, "test"), 100).unwrap(), "[agent] hi");
        assert_eq!(get_transcript_line(&call, 100).unwrap(), "[tool-call] bash arguments={}");
        assert_eq!(get_transcript_line(&AgentMessage::Done, 100).unwrap(), "[done]");
    }

    #[test]
    fn transcript_lines_truncate_long_payloads() {
        let line = get_transcript_line(&AgentMessage::text("msg_1", &"a".repeat(50), true, "test"), 10).unwrap();

        assert_eq!(line, format!("[agent] {}... [truncated]", "a".repeat(10)));
    }

    #[test]
    fn tool_result_transcript_uses_result_arguments() {
        let message = AgentMessage::ToolResult {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: r#"["Cargo.toml"]"#.to_string(),
                result: "file contents".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        };

        assert_eq!(get_transcript_line(&message, 100).unwrap(), "[tool-result] coding__read_file: file contents");
    }
}
