use crate::Transcript;
use std::fmt::Write as _;

#[track_caller]
pub fn assert_tool_called(transcript: &Transcript, name: &str) {
    assert!(transcript.tool_called(name), "expected tool `{name}` to be called");
}

#[track_caller]
pub fn assert_tool_call_count(transcript: &Transcript, name: &str, expected: usize) {
    assert_eq!(transcript.tool_call_count(name), expected, "unexpected call count for tool `{name}`");
}

#[track_caller]
pub fn assert_tool_call_with_args(transcript: &Transcript, name: &str, expected: &serde_json::Value) {
    let mut matched = false;
    let mut parse_failures = String::new();

    for call in transcript.tool_calls(name) {
        match call.arguments_json() {
            Ok(actual) if actual == *expected => {
                matched = true;
                break;
            }
            Ok(_) => {}
            Err(error) => {
                let _ = writeln!(parse_failures, "  arguments={} parse_error={error}", call.arguments);
            }
        }
    }

    if matched {
        return;
    }

    let mut message = format!("expected tool `{name}` to be called with args `{expected}`");
    if !parse_failures.is_empty() {
        message.push_str("\nNon-JSON arguments seen for this tool (skipped from match):\n");
        message.push_str(&parse_failures);
    }
    panic!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::{AgentEvent, ToolEvent};
    use llm::ToolCallResult;

    #[test]
    fn assert_tool_call_with_args_accepts_matching_json_arguments() {
        let trace = transcript_with_messages(vec![tool_result("bash", r#"{"command":"pwd"}"#)]);

        assert_tool_call_with_args(&trace, "bash", &serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    #[should_panic(expected = "Non-JSON arguments seen for this tool")]
    fn assert_tool_call_with_args_surfaces_parse_failures() {
        let trace = transcript_with_messages(vec![tool_result("bash", "not json")]);

        assert_tool_call_with_args(&trace, "bash", &serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    #[should_panic(expected = "expected tool `missing` to be called")]
    fn assert_tool_called_panics_when_tool_was_not_called() {
        let trace = transcript_with_messages(vec![]);

        assert_tool_called(&trace, "missing");
    }

    fn transcript_with_messages(messages: Vec<AgentEvent>) -> Transcript {
        Transcript::new(messages)
    }

    fn tool_result(name: &str, arguments: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: name.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        })
    }
}
