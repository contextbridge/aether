use crate::EvalReport;
use std::fmt::Write as _;

#[track_caller]
pub fn assert_tool_called(report: &EvalReport, name: &str) {
    assert!(report.tool_called(name), "expected tool `{name}` to be called\n\n{}", report.failure_context());
}

#[track_caller]
pub fn assert_tool_call_count(report: &EvalReport, name: &str, expected: usize) {
    assert_eq!(
        report.tool_call_count(name),
        expected,
        "unexpected call count for tool `{name}`\n\n{}",
        report.failure_context()
    );
}

#[track_caller]
pub fn assert_tool_call_with_args(report: &EvalReport, name: &str, expected: &serde_json::Value) {
    let mut matched = false;
    let mut parse_failures = String::new();

    for call in report.tool_calls(name) {
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
    panic!("{message}\n\n{}", report.failure_context());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvalMessage, Workspace};

    #[test]
    fn assert_tool_call_with_args_accepts_matching_json_arguments() {
        let report = report_with_messages(vec![AgentEvalMessage::ToolCall {
            name: "bash".to_string(),
            arguments: r#"{"command":"pwd"}"#.to_string(),
        }]);

        assert_tool_call_with_args(&report, "bash", &serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    #[should_panic(expected = "Non-JSON arguments seen for this tool")]
    fn assert_tool_call_with_args_surfaces_parse_failures() {
        let report = report_with_messages(vec![AgentEvalMessage::ToolCall {
            name: "bash".to_string(),
            arguments: "not json".to_string(),
        }]);

        assert_tool_call_with_args(&report, "bash", &serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    #[should_panic(expected = "expected tool `missing` to be called")]
    fn assert_tool_called_panics_when_tool_was_not_called() {
        let report = report_with_messages(vec![]);

        assert_tool_called(&report, "missing");
    }

    fn report_with_messages(messages: Vec<AgentEvalMessage>) -> EvalReport {
        EvalReport::new("prompt".to_string(), Workspace::empty().unwrap(), messages, None, None)
    }
}
