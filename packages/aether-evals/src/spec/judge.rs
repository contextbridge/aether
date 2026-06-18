use super::report::{JudgeCriterionSummary, JudgeSummary};
use super::types::{Expect, JudgeCriterionSpec, JudgeSpec};
use crate::TaskRun;
use crate::agents::truncate_chars;
use crate::evals::format_transcript;
use aether_core::events::AgentMessage;
use futures::StreamExt;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use schemars::{JsonSchema, Schema, schema_for};
use serde::Deserialize;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const JUDGE_CONTEXT_CHARS: usize = 4_000;

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

#[derive(Debug, Clone, Default)]
pub struct JudgeContext {
    pub transcript: Option<Vec<AgentMessage>>,
    pub diff: Option<String>,
    pub files: BTreeMap<String, String>,
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

pub fn judge() -> JudgeBuilder {
    JudgeBuilder::default()
}

impl Judge {
    pub fn response_schema() -> Schema {
        JudgeRubricResponse::schema()
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
        let prompt = build_prompt(self.instructions.unwrap_or_default(), task, &self.context, &criteria);
        Ok(Judge { prompt, criteria })
    }
}

pub(crate) struct JudgeRunner<'a> {
    llm: &'a dyn StreamingModelProvider,
    judge: Judge,
}

impl<'a> JudgeRunner<'a> {
    pub(crate) fn from_eval_run(
        llm: &'a dyn StreamingModelProvider,
        run: &TaskRun,
        expect: &Expect,
        spec: &JudgeSpec,
    ) -> Result<Self, JudgeError> {
        let builder = judge()
            .instructions(spec.instructions.clone().unwrap_or_default())
            .task(run.prompt().to_string())
            .transcript(run.transcript().messages().to_vec())
            .criteria(&spec.criteria)
            .files(collect_eval_context_files(run, expect, spec));

        let builder = match run.workspace().capture_git_diffs().0 {
            Some(diff) => builder.diff(truncate_chars(&diff.diff, JUDGE_CONTEXT_CHARS)),
            None => builder,
        };

        Ok(Self { llm, judge: builder.build()? })
    }

    pub(crate) async fn run(&self) -> Result<JudgeSummary, JudgeError> {
        tracing::info!("Running LLM judge");
        let raw_response = self.stream_response().await?;
        let response: JudgeRubricResponse = serde_json::from_str(extract_json_object(&raw_response))
            .map_err(|source| JudgeError::InvalidJson { source, raw_response: raw_response.clone() })?;
        self.judge.summarize(response)
    }

    async fn stream_response(&self) -> Result<String, JudgeError> {
        let message = ChatMessage::User {
            content: vec![ContentBlock::text(self.judge.prompt.clone())],
            timestamp: IsoString::now(),
        };
        let mut response_stream = self.llm.stream_response(&Context::new(vec![message], vec![]));
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

impl JudgeRubricResponse {
    pub fn schema() -> Schema {
        schema_for!(Self)
    }
}

fn build_prompt(instructions: String, task: String, context: &JudgeContext, criteria: &[JudgeCriterionSpec]) -> String {
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

fn collect_eval_context_files(run: &TaskRun, expect: &Expect, spec: &JudgeSpec) -> BTreeMap<String, String> {
    expect
        .files
        .keys()
        .chain(expect.files_contain.keys())
        .chain(spec.context_files.iter())
        .map(|path| {
            let contents = std::fs::read_to_string(run.workspace().join(path))
                .map(|contents| truncate_chars(&contents, JUDGE_CONTEXT_CHARS))
                .unwrap_or_else(|error| format!("could not read: {error}"));
            (path.clone(), contents)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use std::fs::write;
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::{GitRepoSpec, Transcript, Workspace};
    use llm::testing::FakeLlmProvider;
    use llm::{ChatMessage, LlmError, ToolCallRequest};

    const VALID_RESPONSE: &str = r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"correct"},{"id":"clarity","score":0.5,"reason":"brief"}],"overall_reason":"good"}"#;

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
    fn judge_builder_accepts_slice_criteria() {
        let criteria = vec![criterion("behavior", "does the thing", true, 1.0, 0.8)];
        let judge = judge().task("do it").criteria(&criteria).build().unwrap();
        assert_eq!(judge.criteria[0].id, "behavior");
    }

    #[test]
    fn judge_summarizes_weighted_rubric() {
        let judge = judge().task("prompt").criteria(judge_spec().criteria).build().unwrap();

        let summary = judge.summarize(serde_json::from_str(VALID_RESPONSE).unwrap()).unwrap();

        assert!(summary.passed);
        assert!((summary.score - 0.875).abs() < f64::EPSILON);
        assert!((summary.criteria[1].score - 0.5).abs() < f64::EPSILON);
        assert!(summary.reason.contains("all blockers met"));
    }

    #[test]
    fn judge_zeroes_score_when_blocker_fails() {
        let judge = judge().task("prompt").criteria(judge_spec().criteria).build().unwrap();
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
        let judge = judge().task("prompt").criteria(judge_spec().criteria).build().unwrap();
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

    #[tokio::test]
    async fn judge_runner_extracts_json_object_from_surrounding_prose() {
        let response = format!("Here is my assessment:\n{VALID_RESPONSE}");
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(&response)]);

        let summary = JudgeRunner::from_eval_run(&judge_llm, &run(), &Expect::default(), &judge_spec())
            .unwrap()
            .run()
            .await
            .unwrap();

        assert!(summary.passed);
    }

    #[tokio::test]
    async fn judge_runner_returns_invalid_json_error_with_raw_response() {
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text("not json")]);

        let error = JudgeRunner::from_eval_run(&judge_llm, &run(), &Expect::default(), &judge_spec())
            .unwrap()
            .run()
            .await
            .unwrap_err();

        let JudgeError::InvalidJson { raw_response, .. } = error else {
            panic!("expected InvalidJson, got {error:?}");
        };
        assert_eq!(raw_response, "not json");
    }

    #[tokio::test]
    async fn judge_runner_returns_stream_error_on_llm_failure() {
        let judge_llm = FakeLlmProvider::from_results(vec![vec![Err(LlmError::Other("boom".to_string()))]]);

        let error = JudgeRunner::from_eval_run(&judge_llm, &run(), &Expect::default(), &judge_spec())
            .unwrap()
            .run()
            .await
            .unwrap_err();

        assert!(matches!(error, JudgeError::Stream(_)));
        assert!(error.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn judge_runner_prompt_includes_eval_run_context() {
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
        let run = TaskRun::new("edit the file".to_string(), Workspace::empty().unwrap(), Transcript::new(messages));
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        JudgeRunner::from_eval_run(&judge_llm, &run, &Expect::default(), &judge_spec()).unwrap().run().await.unwrap();

        let prompt = judged_prompt(&judge_llm);
        assert!(prompt.contains("Grade maintainability."));
        assert!(prompt.contains("behavior"));
        assert!(prompt.contains("The behavior is correct."));
        assert!(prompt.contains("edit the file"));
        assert!(prompt.contains("[tool-call] bash"));
        assert!(prompt.contains("[agent] all done"));
        assert!(prompt.contains("overall_reason"));
    }

    #[tokio::test]
    async fn judge_runner_prompt_includes_agent_diff_from_workspace() {
        let workspace = git_workspace_with_agent_change();
        let run = TaskRun::new("p".to_string(), workspace, Transcript::new(vec![AgentMessage::Done]));
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        JudgeRunner::from_eval_run(&judge_llm, &run, &Expect::default(), &judge_spec()).unwrap().run().await.unwrap();

        let prompt = judged_prompt(&judge_llm);
        assert!(prompt.contains("## Git diff"));
        assert!(prompt.contains("+agent change"));
    }

    #[tokio::test]
    async fn judge_runner_prompt_includes_final_contents_of_files_under_evaluation() {
        let workspace =
            Workspace::from_files([("notes.txt", "beta\nalpha\n"), ("extra.txt", "extra context")]).unwrap();
        let run = TaskRun::new("p".to_string(), workspace, Transcript::new(vec![AgentMessage::Done]));
        let expect =
            Expect { files_contain: [("notes.txt".to_string(), "beta".to_string())].into(), ..Expect::default() };
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        JudgeRunner::from_eval_run(&judge_llm, &run, &expect, &judge_spec()).unwrap().run().await.unwrap();

        let prompt = judged_prompt(&judge_llm);
        assert!(prompt.contains("<path>notes.txt</path>"));
        assert!(prompt.contains("beta\nalpha"));
        assert!(prompt.contains("<path>extra.txt</path>"));
        assert!(prompt.contains("extra context"));
    }

    fn run() -> TaskRun {
        TaskRun::new("prompt".to_string(), Workspace::empty().unwrap(), Transcript::new(vec![AgentMessage::Done]))
    }

    fn criterion(id: &str, description: &str, blocking: bool, weight: f64, threshold: f64) -> JudgeCriterionSpec {
        JudgeCriterionSpec { id: id.to_string(), description: description.to_string(), blocking, weight, threshold }
    }

    fn git_workspace_with_agent_change() -> Workspace {
        let source = tempfile::tempdir().unwrap();
        git(source.path(), ["init", "--initial-branch", "main"]);
        git(source.path(), ["config", "user.email", "eval@example.com"]);
        git(source.path(), ["config", "user.name", "Eval"]);
        std::fs::write(source.path().join("notes.txt"), "start\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "-m", "start"]);
        let start_commit = git_output(source.path(), ["rev-parse", "HEAD"]);
        std::fs::write(source.path().join("notes.txt"), "gold\n").unwrap();
        git(source.path(), ["commit", "-am", "gold"]);
        let gold_commit = git_output(source.path(), ["rev-parse", "HEAD"]);

        let workspace = Workspace::from_git_repo(GitRepoSpec {
            url: source.path().to_string_lossy().to_string(),
            start_commit,
            gold_commit,
            subdir: None,
        })
        .unwrap();
        write(workspace.join("notes.txt"), "start\nagent change\n").unwrap();
        workspace
    }

    fn git<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git").args(args).current_dir(cwd).output().unwrap();
        assert!(output.status.success(), "git failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> String {
        let output = Command::new("git").args(args).current_dir(cwd).output().unwrap();
        assert!(output.status.success(), "git failed: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn judge_spec() -> JudgeSpec {
        serde_json::from_str(
            r#"{
                "model": "judge:model",
                "instructions": "Grade maintainability.",
                "contextFiles": ["extra.txt"],
                "criteria": [
                    { "id": "behavior", "description": "The behavior is correct.", "blocking": true, "weight": 3.0, "threshold": 1.0 },
                    { "id": "clarity", "description": "The response is clear.", "blocking": false, "weight": 1.0, "threshold": 0.5 }
                ]
            }"#,
        )
        .unwrap()
    }

    fn judged_prompt(judge_llm: &FakeLlmProvider) -> String {
        let contexts = judge_llm.captured_contexts();
        let contexts = contexts.lock().unwrap();
        let ChatMessage::User { content, .. } = &contexts[0].messages()[0] else {
            panic!("expected a user message in the judge context");
        };
        ContentBlock::join_text(content)
    }
}
