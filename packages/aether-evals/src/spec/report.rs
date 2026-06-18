use super::runner::WorkspaceRetention;
use crate::evals::{RetainedWorkspaceInfo, TaskRun};
use aether_core::events::AgentMessage;
use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvalStreamEvent {
    AgentMessage { message: AgentMessage },
    Outcome { outcome: EvalOutcome },
}

impl EvalStreamEvent {
    pub fn schema() -> Schema {
        schema_for!(Self)
    }
}

/// Outcome of running a collection of eval files.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalFilesReport {
    pub evals: Vec<EvalOutcome>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalOutcome {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub tool_calls: Vec<EvalToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_context: Option<String>,
    /// Retained workspace root and effective cwd, when the eval was run with retention. The caller owns cleanup of `root_path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_workspace: Option<RetainedWorkspaceInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalToolCall {
    pub name: String,
    pub arguments: Option<Value>,
    pub raw_arguments: String,
}

impl EvalOutcome {
    /// JSON schema for a single eval's outcome (the output returned to out-of-process callers).
    pub fn schema() -> Schema {
        schema_for!(Self)
    }

    pub(crate) fn setup_failed(name: impl Into<String>, error: impl std::fmt::Display) -> Self {
        Self::failed(name, vec![format!("workspace setup failed: {error}")])
    }

    pub(crate) fn failed(name: impl Into<String>, failures: Vec<String>) -> Self {
        Self {
            name: name.into(),
            passed: false,
            failures,
            tool_calls: Vec::new(),
            judge: None,
            failure_context: None,
            retained_workspace: None,
        }
    }

    pub(crate) fn from_task_run(
        name: impl Into<String>,
        run: TaskRun,
        failures: Vec<String>,
        judge: Option<JudgeSummary>,
        retention: WorkspaceRetention,
    ) -> Self {
        let passed = failures.is_empty();
        let failure_context = (!passed).then(|| run.failure_context());
        let tool_calls = EvalToolCall::from_task_run(&run);
        let retained_workspace = match retention {
            WorkspaceRetention::Retain => Some(run.into_workspace().persist()),
            WorkspaceRetention::Discard => None,
        };

        Self { name: name.into(), passed, failures, tool_calls, judge, failure_context, retained_workspace }
    }
}

impl EvalToolCall {
    fn from_task_run(run: &TaskRun) -> Vec<Self> {
        run.transcript()
            .all_tool_calls()
            .map(|call| Self {
                name: call.name.to_string(),
                arguments: call.arguments_json().ok(),
                raw_arguments: call.arguments.to_string(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgeSummary {
    pub passed: bool,
    pub score: f64,
    pub reason: String,
    pub criteria: Vec<JudgeCriterionSummary>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgeCriterionSummary {
    pub id: String,
    pub description: String,
    pub blocking: bool,
    pub weight: f64,
    pub threshold: f64,
    pub score: f64,
    pub passed: bool,
    pub reason: String,
}

impl EvalFilesReport {
    pub fn passed(&self) -> bool {
        self.evals.iter().all(|eval| eval.passed)
    }

    pub fn passed_count(&self) -> usize {
        self.evals.iter().filter(|eval| eval.passed).count()
    }

    pub fn failed_count(&self) -> usize {
        self.evals.iter().filter(|eval| !eval.passed).count()
    }
}

impl JudgeSummary {
    /// Failure messages for blocking criteria that scored below their threshold.
    pub fn blocking_failures(&self) -> impl Iterator<Item = String> + '_ {
        self.criteria
            .iter()
            .filter(|criterion| criterion.blocking && !criterion.passed)
            .map(|criterion| format!("judge criterion `{}`: {}", criterion.id, criterion.reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::AgentMessage;

    #[test]
    fn blocking_failures_report_only_blocking_criteria_below_threshold() {
        let criterion = |id: &str, blocking, score: f64| JudgeCriterionSummary {
            id: id.to_string(),
            description: "desc".to_string(),
            blocking,
            weight: 1.0,
            threshold: 0.8,
            score,
            passed: score >= 0.8,
            reason: format!("{id} reason"),
        };
        let summary = JudgeSummary {
            passed: false,
            score: 0.0,
            reason: "r".to_string(),
            criteria: vec![
                criterion("met", true, 0.9),
                criterion("failed", true, 0.5),
                criterion("advisory", false, 0.0),
            ],
        };

        let failures: Vec<String> = summary.blocking_failures().collect();

        assert_eq!(failures, vec!["judge criterion `failed`: failed reason".to_string()]);
    }

    #[test]
    fn agent_message_event_serializes_with_snake_case_tag_and_nested_message() {
        let event = EvalStreamEvent::AgentMessage {
            message: AgentMessage::Text {
                message_id: "msg_1".to_string(),
                chunk: "hi".to_string(),
                is_complete: false,
                model_name: "fake".to_string(),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "agent_message");
        assert_eq!(json["message"]["type"], "text");
        assert_eq!(json["message"]["message_id"], "msg_1");
        assert_eq!(json["message"]["is_complete"], false);
    }

    #[test]
    fn outcome_event_serializes_with_nested_outcome_fields() {
        let outcome = EvalOutcome {
            name: "edit-notes".to_string(),
            passed: true,
            failures: Vec::new(),
            tool_calls: Vec::new(),
            judge: None,
            failure_context: None,
            retained_workspace: None,
        };
        let event = EvalStreamEvent::Outcome { outcome };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "outcome");
        assert_eq!(json["outcome"]["name"], "edit-notes");
        assert_eq!(json["outcome"]["passed"], true);
    }
}
