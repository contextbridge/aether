use aether_core::events::AgentMessage;
use futures::StreamExt;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use thiserror::Error;

const TRANSCRIPT_PAYLOAD_CHARS: usize = 2_000;

/// Start building an LLM-as-judge from structured context and rubric criteria.
pub fn judge() -> JudgeBuilder {
    JudgeBuilder::default()
}

/// A built judge: the assembled prompt plus the normalized rubric it grades against. Run it with
/// [`Judge::run`] against a model, or grade a parsed [`JudgeRubricResponse`] with
/// [`Judge::summarize`].
#[derive(Debug, Clone)]
pub struct Judge {
    pub prompt: String,
    pub criteria: Vec<JudgeCriterionSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct JudgeBuilder {
    instructions: Option<String>,
    task: Option<String>,
    context: JudgeContext,
    criteria: Vec<JudgeCriterionSpec>,
}

/// Evidence the judge grades against: the agent transcript, a workspace diff, and/or final files.
#[derive(Debug, Clone, Default)]
pub struct JudgeContext {
    pub transcript: Option<Vec<AgentMessage>>,
    pub diff: Option<String>,
    pub files: BTreeMap<String, String>,
}

/// A single rubric criterion scored on a normalized 0.0..=1.0 scale.
#[derive(Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgeCriterionSpec {
    pub id: String,
    pub description: String,
    #[serde(default = "default_blocking")]
    pub blocking: bool,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}

/// The graded result of running a judge: an overall pass/score plus per-criterion detail.
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

/// The raw rubric response the judge model is expected to return.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeRubricResponse {
    pub criteria: Vec<JudgeCriterionResponse>,
    pub overall_reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeCriterionResponse {
    pub id: String,
    pub score: f64,
    pub reason: String,
}

#[derive(Debug, Error)]
pub enum JudgeError {
    #[error("invalid judge input: {0}")]
    InvalidInput(String),

    #[error("judge LLM stream error: {0}")]
    Stream(#[from] llm::LlmError),

    #[error("judge returned invalid JSON: {source}\nRaw response: {raw_response}")]
    InvalidJson {
        #[source]
        source: serde_json::Error,
        raw_response: String,
    },

    #[error("judge returned invalid judgment: {reason}\nRaw response: {raw_response}")]
    InvalidJudgment { reason: String, raw_response: String },
}

impl Judge {
    pub fn response_schema() -> Schema {
        JudgeRubricResponse::schema()
    }

    /// Grade `llm` against this judge's rubric: stream the model's response, parse it as a
    /// [`JudgeRubricResponse`], and summarize it.
    pub async fn run(&self, llm: &dyn StreamingModelProvider) -> Result<JudgeSummary, JudgeError> {
        tracing::info!("Running LLM judge");
        let raw_response = self.stream_response(llm).await?;
        let response: JudgeRubricResponse = serde_json::from_str(extract_json_object(&raw_response))
            .map_err(|source| JudgeError::InvalidJson { source, raw_response: raw_response.clone() })?;
        self.summarize(response)
    }

    pub fn summarize(&self, response: JudgeRubricResponse) -> Result<JudgeSummary, JudgeError> {
        let mut responses = BTreeMap::new();
        for criterion in response.criteria {
            let id = criterion.id.clone();
            if responses.insert(id.clone(), criterion).is_some() {
                return Err(invalid_judgment(format!("duplicate response criterion id `{id}`"), ""));
            }
        }

        let mut summaries = Vec::with_capacity(self.criteria.len());
        let mut weighted_score = 0.0;
        let mut total_weight = 0.0;
        let mut blocking_failed = false;

        for criterion in &self.criteria {
            let Some(response) = responses.remove(&criterion.id) else {
                return Err(invalid_judgment(format!("missing response criterion `{}`", criterion.id), ""));
            };
            if !response.score.is_finite() || !(0.0..=1.0).contains(&response.score) {
                return Err(invalid_judgment(
                    format!("criterion `{}` score must be between 0.0 and 1.0", criterion.id),
                    "",
                ));
            }

            let passed = response.score >= criterion.threshold;
            blocking_failed |= criterion.blocking && !passed;
            weighted_score += response.score * criterion.weight;
            total_weight += criterion.weight;
            summaries.push(JudgeCriterionSummary {
                id: criterion.id.clone(),
                description: criterion.description.clone(),
                blocking: criterion.blocking,
                weight: criterion.weight,
                threshold: criterion.threshold,
                score: response.score,
                passed,
                reason: response.reason,
            });
        }

        if let Some(id) = responses.keys().next() {
            return Err(invalid_judgment(format!("unknown response criterion `{id}`"), ""));
        }

        let weighted_score = weighted_score / total_weight;
        let score = if blocking_failed { 0.0 } else { weighted_score };
        let reason = if blocking_failed {
            format!("weighted score {:.2}; one or more blockers failed; {}", weighted_score, response.overall_reason)
        } else {
            format!("weighted score {:.2}; all blockers met; {}", weighted_score, response.overall_reason)
        };

        Ok(JudgeSummary { passed: !blocking_failed, score, reason, criteria: summaries })
    }

    async fn stream_response(&self, llm: &dyn StreamingModelProvider) -> Result<String, JudgeError> {
        let message =
            ChatMessage::User { content: vec![ContentBlock::text(self.prompt.clone())], timestamp: IsoString::now() };
        let mut response_stream = llm.stream_response(&Context::new(vec![message], vec![]));
        let mut raw_response = String::new();
        while let Some(result) = response_stream.next().await {
            match result {
                Ok(LlmResponse::Text { chunk }) => raw_response.push_str(&chunk),
                Err(error) => return Err(JudgeError::Stream(error)),
                _ => {}
            }
        }
        Ok(raw_response)
    }
}

impl JudgeBuilder {
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    pub fn transcript(mut self, transcript: impl Into<Vec<AgentMessage>>) -> Self {
        self.context.transcript = Some(transcript.into());
        self
    }

    pub fn diff(mut self, diff: impl Into<String>) -> Self {
        self.context.diff = Some(diff.into());
        self
    }

    pub fn file(mut self, path: impl Into<String>, contents: impl Into<String>) -> Self {
        self.context.files.insert(path.into(), contents.into());
        self
    }

    pub fn files<T, U, V>(mut self, files: T) -> Self
    where
        T: IntoIterator<Item = (U, V)>,
        U: Into<String>,
        V: Into<String>,
    {
        self.context.files.extend(files.into_iter().map(|(path, contents)| (path.into(), contents.into())));
        self
    }

    pub fn criteria<T, U>(mut self, criteria: T) -> Self
    where
        T: IntoIterator<Item = U>,
        U: Borrow<JudgeCriterionSpec>,
    {
        self.criteria = criteria.into_iter().map(|criterion| criterion.borrow().clone()).collect();
        self
    }

    pub fn context(mut self, context: JudgeContext) -> Self {
        self.context = context;
        self
    }

    pub fn build(self) -> Result<Judge, JudgeError> {
        let task = self.task.ok_or_else(|| JudgeError::InvalidInput("judge task must be provided".to_string()))?;
        let criteria = normalize_criteria(self.criteria)?;
        let prompt = build_prompt(&self.instructions.unwrap_or_default(), &task, &self.context, &criteria);
        Ok(Judge { prompt, criteria })
    }
}

impl JudgeCriterionSpec {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            blocking: default_blocking(),
            weight: default_weight(),
            threshold: default_threshold(),
        }
    }

    pub fn blocking(mut self, blocking: bool) -> Self {
        self.blocking = blocking;
        self
    }

    pub fn weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }

    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
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

impl JudgeRubricResponse {
    pub fn schema() -> Schema {
        schema_for!(Self)
    }
}

fn build_prompt(instructions: &str, task: &str, context: &JudgeContext, criteria: &[JudgeCriterionSpec]) -> String {
    let mut sections = vec![
        format!("## Instructions\n\n{instructions}"),
        format!("## Task\n\nThe agent you're evaluating was given this task: <task>{task}</task>"),
    ];

    if let Some(transcript) = &context.transcript
        && !transcript.is_empty()
    {
        sections.push(format!(
            "## Agent Transcript\n\nTranscript of the agent you're evaluating: <transcript>{}</transcript>",
            format_transcript(transcript)
        ));
    }

    if let Some(diff) = &context.diff
        && !diff.is_empty()
    {
        sections.push(format!("## Git diff\n\nGit diff produced by the agent you're evaluating: <diff>{diff}</diff>"));
    }

    if !context.files.is_empty() {
        let blocks = context
            .files
            .iter()
            .map(|(path, contents)| format!("<file><path>{path}</path><contents>{contents}</contents></file>"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## File Contents\n\nFiles under evaluation: <files>{blocks}</files>"));
    }

    let rubric = criteria
        .iter()
        .map(|criterion| {
            format!(
                "- id: {}\n  blocking: {}\n  weight: {}\n  threshold: {}\n  description: {}",
                criterion.id, criterion.blocking, criterion.weight, criterion.threshold, criterion.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    sections.push(format!("## Rubric criteria\n\n{rubric}"));
    sections.push(format!(
        "{}\n{}\n{}\n{}",
        "Return exactly one result for every criterion ID above and no extra criteria.",
        "Scores must be normalized numbers from 0.0 to 1.0.",
        "Respond with ONLY a JSON object matching this schema:",
        judge_response_schema()
    ));

    sections.join("\n\n")
}

fn normalize_criteria(criteria: Vec<JudgeCriterionSpec>) -> Result<Vec<JudgeCriterionSpec>, JudgeError> {
    if criteria.is_empty() {
        return Err(JudgeError::InvalidInput("judge criteria must not be empty".to_string()));
    }

    let mut ids = BTreeSet::new();
    let mut normalized = Vec::with_capacity(criteria.len());
    for mut criterion in criteria {
        criterion.id = criterion.id.trim().to_string();
        if criterion.id.is_empty() {
            return Err(JudgeError::InvalidInput("judge criterion id must not be empty".to_string()));
        }
        if !ids.insert(criterion.id.clone()) {
            return Err(JudgeError::InvalidInput(format!("duplicate judge criterion id `{}`", criterion.id)));
        }
        if criterion.description.trim().is_empty() {
            return Err(JudgeError::InvalidInput(format!(
                "judge criterion `{}` description must not be empty",
                criterion.id
            )));
        }
        if !criterion.weight.is_finite() || criterion.weight <= 0.0 {
            return Err(JudgeError::InvalidInput(format!(
                "judge criterion `{}` weight must be positive and finite",
                criterion.id
            )));
        }
        if !criterion.threshold.is_finite() || !(0.0..=1.0).contains(&criterion.threshold) {
            return Err(JudgeError::InvalidInput(format!(
                "judge criterion `{}` threshold must be between 0.0 and 1.0",
                criterion.id
            )));
        }
        normalized.push(criterion);
    }
    Ok(normalized)
}

fn extract_json_object(response: &str) -> &str {
    let trimmed = response.trim();
    match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if start <= end => &trimmed[start..=end],
        _ => trimmed,
    }
}

fn invalid_judgment(reason: String, raw_response: &str) -> JudgeError {
    JudgeError::InvalidJudgment { reason, raw_response: raw_response.to_string() }
}

fn judge_response_schema() -> String {
    serde_json::to_string_pretty(&JudgeRubricResponse::schema()).unwrap()
}

fn default_blocking() -> bool {
    true
}

fn default_weight() -> f64 {
    1.0
}

fn default_threshold() -> f64 {
    1.0
}

fn format_transcript(messages: &[AgentMessage]) -> String {
    let mut transcript = String::new();
    for message in messages {
        if let Some(line) = get_transcript_line(message, TRANSCRIPT_PAYLOAD_CHARS) {
            let _ = writeln!(transcript, "{line}");
        }
    }
    transcript
}

fn get_transcript_line(message: &AgentMessage, max_payload_chars: usize) -> Option<String> {
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}... [truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::AgentMessage;
    use llm::testing::FakeLlmProvider;
    use llm::{LlmError, ToolCallRequest, ToolCallResult};

    const VALID_RESPONSE: &str = r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"correct"},{"id":"clarity","score":0.5,"reason":"brief"}],"overall_reason":"good"}"#;

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

    #[test]
    fn judge_builder_builds_prompt_from_context_and_criteria() {
        let judge = judge()
            .instructions("be strict")
            .task("do the thing")
            .diff("+added line")
            .file("notes.txt", "beta\n")
            .criteria([criterion("works", "the task works", true, 2.0, 0.9)])
            .build()
            .unwrap();

        assert!(judge.prompt.contains("## Instructions\n\nbe strict"));
        assert!(judge.prompt.contains("## Task"));
        assert!(judge.prompt.contains("The agent you're evaluating was given this task: <task>do the thing</task>"));
        assert!(judge.prompt.contains("## Git diff"));
        assert!(judge.prompt.contains("Git diff produced by the agent you're evaluating: <diff>+added line</diff>"));
        assert!(judge.prompt.contains("## File Contents"));
        assert!(judge.prompt.contains("<path>notes.txt</path>"));
        assert!(judge.prompt.contains("<contents>beta\n</contents>"));
        assert!(judge.prompt.contains("## Rubric criteria"));
        assert!(judge.prompt.contains("blocking: true"));
        assert!(judge.prompt.contains("threshold: 0.9"));
        assert!(judge.prompt.contains("weight: 2"));
        assert!(judge.prompt.contains("Return exactly one result for every criterion ID above and no extra criteria."));
        assert!(judge.prompt.contains("Respond with ONLY a JSON object matching this schema:"));
    }

    #[test]
    fn judge_builder_renders_transcript_context() {
        let messages = vec![
            AgentMessage::ToolCall {
                request: ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                },
                model_name: "test".to_string(),
            },
            AgentMessage::text("msg_1", "all done", true, "test"),
        ];

        let judge = judge()
            .task("edit the file")
            .transcript(messages)
            .criteria([criterion("behavior", "did it work", true, 1.0, 1.0)])
            .build()
            .unwrap();

        assert!(judge.prompt.contains("## Agent Transcript"));
        assert!(judge.prompt.contains("[tool-call] bash"));
        assert!(judge.prompt.contains("[agent] all done"));
    }

    #[test]
    fn judge_builder_accepts_slice_criteria() {
        let criteria = vec![criterion("behavior", "does the thing", true, 1.0, 0.8)];
        let judge = judge().task("do it").criteria(&criteria).build().unwrap();
        assert_eq!(judge.criteria[0].id, "behavior");
    }

    #[test]
    fn judge_summarizes_weighted_rubric() {
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();

        let summary = judge.summarize(serde_json::from_str(VALID_RESPONSE).unwrap()).unwrap();

        assert!(summary.passed);
        assert!((summary.score - 0.875).abs() < f64::EPSILON);
        assert!((summary.criteria[1].score - 0.5).abs() < f64::EPSILON);
        assert!(summary.reason.contains("all blockers met"));
    }

    #[test]
    fn judge_zeroes_score_when_blocker_fails() {
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();
        let response = serde_json::from_str(
            r#"{"criteria":[{"id":"behavior","score":0.75,"reason":"wrong behavior"},{"id":"clarity","score":1.0,"reason":"clear"}],"overall_reason":"bad"}"#,
        )
        .unwrap();

        let summary = judge.summarize(response).unwrap();

        assert!(!summary.passed);
        assert!(summary.score.abs() < f64::EPSILON);
        assert!(!summary.criteria[0].passed);
        assert!(summary.reason.contains("one or more blockers failed"));
    }

    #[test]
    fn judge_rejects_invalid_criterion_sets() {
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();
        for raw_response in [
            r#"{"criteria":[],"overall_reason":"missing"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"ok"},{"id":"behavior","score":1.0,"reason":"ok"}],"overall_reason":"duplicate"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"ok"},{"id":"clarity","score":1.0,"reason":"ok"},{"id":"extra","score":1.0,"reason":"ok"}],"overall_reason":"unknown"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.5,"reason":"bad"},{"id":"clarity","score":1.0,"reason":"ok"}],"overall_reason":"score"}"#,
        ] {
            let response = serde_json::from_str(raw_response).unwrap();

            let error = judge.summarize(response).unwrap_err();

            assert!(matches!(error, JudgeError::InvalidJudgment { .. }), "response: {raw_response}");
        }
    }

    #[test]
    fn judge_builder_rejects_invalid_inputs() {
        for (builder, expected) in [
            (judge().criteria([criterion("behavior", "ok", true, 1.0, 0.8)]), "judge task must be provided"),
            (judge().task("prompt"), "judge criteria must not be empty"),
            (
                judge().task("prompt").criteria([criterion("", "ok", true, 1.0, 0.8)]),
                "judge criterion id must not be empty",
            ),
            (
                judge().task("prompt").criteria([criterion("behavior", "ok", true, 0.0, 0.8)]),
                "weight must be positive and finite",
            ),
        ] {
            let error = builder.build().unwrap_err();
            assert!(error.to_string().contains(expected), "got: {error}");
        }
    }

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

    #[tokio::test]
    async fn judge_run_extracts_json_object_from_surrounding_prose() {
        let response = format!("Here is my assessment:\n{VALID_RESPONSE}");
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(&response)]);
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();

        let summary = judge.run(&judge_llm).await.unwrap();

        assert!(summary.passed);
    }

    #[tokio::test]
    async fn judge_run_returns_invalid_json_error_with_raw_response() {
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text("not json")]);
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();

        let error = judge.run(&judge_llm).await.unwrap_err();

        let JudgeError::InvalidJson { raw_response, .. } = error else {
            panic!("expected InvalidJson, got {error:?}");
        };
        assert_eq!(raw_response, "not json");
    }

    #[tokio::test]
    async fn judge_run_returns_stream_error_on_llm_failure() {
        let judge_llm = FakeLlmProvider::from_results(vec![vec![Err(LlmError::Other("boom".to_string()))]]);
        let judge = judge().task("prompt").criteria(default_criteria()).build().unwrap();

        let error = judge.run(&judge_llm).await.unwrap_err();

        assert!(matches!(error, JudgeError::Stream(_)));
        assert!(error.to_string().contains("boom"));
    }

    fn criterion(id: &str, description: &str, blocking: bool, weight: f64, threshold: f64) -> JudgeCriterionSpec {
        JudgeCriterionSpec { id: id.to_string(), description: description.to_string(), blocking, weight, threshold }
    }

    fn default_criteria() -> Vec<JudgeCriterionSpec> {
        vec![
            criterion("behavior", "The behavior is correct.", true, 3.0, 1.0),
            criterion("clarity", "The response is clear.", false, 1.0, 0.5),
        ]
    }
}
