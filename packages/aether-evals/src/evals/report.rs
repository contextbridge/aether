use crate::Workspace;
use crate::agents::{TRANSCRIPT_PAYLOAD_CHARS, get_transcript_line};
use aether_core::events::AgentMessage;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct EvalReport {
    prompt: String,
    workspace: Workspace,
    messages: Vec<AgentMessage>,
    agent_diff: Option<GitDiff>,
    reference_diff: Option<GitDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    pub diff: String,
    pub stats: DiffStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStats {
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
}

pub struct ToolCall<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

impl EvalReport {
    pub(crate) fn new(
        prompt: String,
        workspace: Workspace,
        messages: Vec<AgentMessage>,
        agent_diff: Option<GitDiff>,
        reference_diff: Option<GitDiff>,
    ) -> Self {
        Self { prompt, workspace, messages, agent_diff, reference_diff }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn path(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.workspace.path().join(relative_path)
    }

    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    pub fn agent_diff(&self) -> Option<&GitDiff> {
        self.agent_diff.as_ref()
    }

    pub fn reference_diff(&self) -> Option<&GitDiff> {
        self.reference_diff.as_ref()
    }

    pub fn tool_calls<'a>(&'a self, name: &'a str) -> impl Iterator<Item = ToolCall<'a>> + 'a {
        self.messages.iter().filter_map(move |message| match message {
            AgentMessage::ToolResult { result, .. } if result.name == name => {
                Some(ToolCall { name: &result.name, arguments: &result.arguments })
            }
            AgentMessage::ToolError { error, .. } if error.name == name => {
                Some(ToolCall { name: &error.name, arguments: error.arguments.as_deref().unwrap_or("") })
            }
            _ => None,
        })
    }

    pub fn tool_called(&self, name: &str) -> bool {
        self.tool_calls(name).next().is_some()
    }

    pub fn tool_call_count(&self, name: &str) -> usize {
        self.tool_calls(name).count()
    }

    pub fn failure_context(&self) -> String {
        let mut summary = String::new();
        let _ = writeln!(summary, "Eval failure context");
        let _ = writeln!(summary, "Workspace: {}", self.workspace.path().display());
        summary.push_str("Prompt:\n");
        push_indented(&mut summary, &self.prompt, 2);
        summary.push('\n');

        if let Some(diff) = &self.agent_diff {
            summary.push_str("Agent diff summary:\n");
            push_diff_stats(&mut summary, diff);
            summary.push('\n');
        }

        if let Some(diff) = &self.reference_diff {
            summary.push_str("Reference diff summary:\n");
            push_diff_stats(&mut summary, diff);
            summary.push('\n');
        }

        summary.push_str("Agent messages:\n");
        if self.messages.is_empty() {
            summary.push_str("  none\n");
        } else {
            push_indented(&mut summary, &format_transcript(&self.messages), 2);
        }

        summary
    }
}

impl ToolCall<'_> {
    pub fn arguments_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.arguments)
    }
}

impl DiffStats {
    pub fn from_diff(diff: &str) -> Self {
        let mut lines_added = 0;
        let mut lines_removed = 0;
        let mut files_changed = 0;

        for line in diff.lines() {
            if line.starts_with("diff --git") {
                files_changed += 1;
            } else if line.starts_with('+') && !line.starts_with("+++") {
                lines_added += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                lines_removed += 1;
            }
        }

        Self { files_changed, lines_added, lines_removed }
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

fn push_indented(output: &mut String, value: &str, spaces: usize) {
    let indentation = " ".repeat(spaces);
    for line in value.lines() {
        output.push_str(&indentation);
        output.push_str(line);
        output.push('\n');
    }
}

fn push_diff_stats(output: &mut String, diff: &GitDiff) {
    let _ = writeln!(output, "  Files changed: {}", diff.stats.files_changed);
    let _ = writeln!(output, "  Lines added: {}", diff.stats.lines_added);
    let _ = writeln!(output, "  Lines removed: {}", diff.stats.lines_removed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{ToolCallRequest, ToolCallResult};

    #[test]
    fn diff_stats_from_diff_counts_files_and_changed_lines() {
        let diff = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@\n-old\n+new\ndiff --git a/b.txt b/b.txt\n+++ b/b.txt\n+added\n";

        let stats = DiffStats::from_diff(diff);

        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.lines_added, 2);
        assert_eq!(stats.lines_removed, 1);
    }

    #[test]
    fn failure_context_includes_prompt_workspace_messages_and_diff_summary() {
        let report = EvalReport::new(
            "do the thing".to_string(),
            Workspace::empty().unwrap(),
            vec![AgentMessage::text("msg_1", "done", true, "test"), AgentMessage::Done],
            Some(GitDiff {
                diff: "diff --git a/a.txt b/a.txt\n+new\n".to_string(),
                stats: DiffStats { files_changed: 1, lines_added: 1, lines_removed: 0 },
            }),
            None,
        );

        let context = report.failure_context();

        assert!(context.contains("Eval failure context"));
        assert!(context.contains("Workspace:"));
        assert!(context.contains("do the thing"));
        assert!(context.contains("Agent diff summary:"));
        assert!(context.contains("Files changed: 1"));
        assert!(context.contains("[agent] done"));
    }

    #[test]
    fn tool_call_count_counts_matching_tool_calls() {
        let report = report_with_messages(vec![tool_call("bash"), tool_call("read"), tool_result("bash")]);

        assert!(report.tool_called("bash"));
        assert!(!report.tool_called("read"));
        assert!(!report.tool_called("write"));
        assert_eq!(report.tool_call_count("bash"), 1);
        assert_eq!(report.tool_call_count("read"), 0);
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

    pub(crate) fn report_with_messages(messages: Vec<AgentMessage>) -> EvalReport {
        EvalReport::new("prompt".to_string(), Workspace::empty().unwrap(), messages, None, None)
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
