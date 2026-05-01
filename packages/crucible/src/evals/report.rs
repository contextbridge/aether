use crate::evals::judge::{JudgeError, JudgeResult, LlmJudgeContext};
use crate::metrics::EvalMetric;
use crate::{AgentEvalMessage, Workspace};
use futures::StreamExt;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct EvalReport {
    prompt: String,
    workspace: Workspace,
    messages: Vec<AgentEvalMessage>,
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
        messages: Vec<AgentEvalMessage>,
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

    pub fn messages(&self) -> &[AgentEvalMessage] {
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
            AgentEvalMessage::ToolCall { name: call_name, arguments } if call_name == name => {
                Some(ToolCall { name: call_name, arguments })
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
            for message in &self.messages {
                push_message(&mut summary, message);
            }
        }

        summary
    }

    pub async fn judge<T: Fn(&LlmJudgeContext) -> String>(
        &self,
        llm: &dyn StreamingModelProvider,
        build_prompt: T,
    ) -> Result<JudgeResult, JudgeError> {
        tracing::info!("Running LLM judge");
        let prompt = ChatMessage::User {
            content: vec![ContentBlock::text(build_prompt(&LlmJudgeContext {
                workspace: &self.workspace,
                original_prompt: &self.prompt,
                messages: &self.messages,
            }))],
            timestamp: IsoString::now(),
        };

        let mut response_stream = llm.stream_response(&Context::new(vec![prompt], vec![]));
        let mut raw_response = String::new();
        while let Some(result) = response_stream.next().await {
            match result {
                Ok(LlmResponse::Text { chunk }) => {
                    raw_response.push_str(&chunk);
                }
                Err(error) => {
                    tracing::error!("LLM judge error: {}", error);
                    return Err(JudgeError::Stream(error));
                }
                _ => {}
            }
        }

        let trimmed_response = raw_response.trim();
        let metric: EvalMetric = serde_json::from_str(trimmed_response)
            .map_err(|e| JudgeError::InvalidJson { source: e, raw_response: raw_response.clone() })?;

        let (passed, reason) = match metric {
            EvalMetric::Binary(binary) => (binary.success, binary.reason),
            EvalMetric::Numeric(numeric) => {
                let success = numeric.score / numeric.max_score >= 0.7;
                (success, format!("{} (score: {}/{})", numeric.reason, numeric.score, numeric.max_score))
            }
        };

        Ok(JudgeResult::new(passed, reason, raw_response))
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

fn push_message(output: &mut String, message: &AgentEvalMessage) {
    match message {
        AgentEvalMessage::AgentText(text) => {
            let _ = writeln!(output, "  [agent] {}", truncate_for_report(text, 2_000));
        }
        AgentEvalMessage::ToolCall { name, arguments } => {
            let _ = writeln!(output, "  [tool-call] {name} arguments={}", truncate_for_report(arguments, 1_000));
        }
        AgentEvalMessage::ToolResult { name, result } => {
            let _ = writeln!(output, "  [tool-result] {name}: {}", truncate_for_report(result, 1_000));
        }
        AgentEvalMessage::ToolError(error) => {
            let _ = writeln!(output, "  [tool-error] {}", truncate_for_report(error, 1_000));
        }
        AgentEvalMessage::Error(error) => {
            let _ = writeln!(output, "  [error] {}", truncate_for_report(error, 1_000));
        }
        AgentEvalMessage::Done => {
            output.push_str("  [done]\n");
        }
    }
}

fn truncate_for_report(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}... [truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;

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
            vec![AgentEvalMessage::AgentText("done".to_string()), AgentEvalMessage::Done],
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
        let report = report_with_messages(vec![
            AgentEvalMessage::ToolCall { name: "bash".to_string(), arguments: "{}".to_string() },
            AgentEvalMessage::ToolCall { name: "read".to_string(), arguments: "{}".to_string() },
            AgentEvalMessage::ToolCall { name: "bash".to_string(), arguments: "{}".to_string() },
        ]);

        assert!(report.tool_called("bash"));
        assert!(!report.tool_called("write"));
        assert_eq!(report.tool_call_count("bash"), 2);
        assert_eq!(report.tool_call_count("read"), 1);
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

    pub(crate) fn report_with_messages(messages: Vec<AgentEvalMessage>) -> EvalReport {
        EvalReport::new("prompt".to_string(), Workspace::empty().unwrap(), messages, None, None)
    }
}
