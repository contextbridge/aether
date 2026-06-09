use super::report::{JudgeCriterionSummary, JudgeSummary};
use super::types::{Expect, JudgeSpec};
use crate::EvalReport;
use crate::agents::truncate_chars;
use crate::evals::format_transcript;
use futures::StreamExt;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use thiserror::Error;

const JUDGE_CONTEXT_CHARS: usize = 4_000;

/// Runs the LLM judge for an eval: builds the rubric prompt from the completed run, streams the
/// judge model's response, and scores it against the rubric.
pub(crate) struct Judge<'a> {
    llm: &'a dyn StreamingModelProvider,
    report: &'a EvalReport,
    expect: &'a Expect,
    spec: &'a JudgeSpec,
}

#[derive(Debug, Error)]
pub enum JudgeError {
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

impl<'a> Judge<'a> {
    pub(crate) fn new(
        llm: &'a dyn StreamingModelProvider,
        report: &'a EvalReport,
        expect: &'a Expect,
        spec: &'a JudgeSpec,
    ) -> Self {
        Self { llm, report, expect, spec }
    }

    pub(crate) async fn run(&self) -> Result<JudgeSummary, JudgeError> {
        tracing::info!("Running LLM judge");
        let raw_response = self.stream_response(self.build_prompt()).await?;
        let response: JudgeRubricResponse = serde_json::from_str(extract_json_object(&raw_response))
            .map_err(|source| JudgeError::InvalidJson { source, raw_response: raw_response.clone() })?;
        self.summarize_response(response, &raw_response)
    }

    async fn stream_response(&self, prompt: String) -> Result<String, JudgeError> {
        let message = ChatMessage::User { content: vec![ContentBlock::text(prompt)], timestamp: IsoString::now() };
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

    fn build_prompt(&self) -> String {
        let mut prompt = String::new();
        let _ = writeln!(prompt, "You are grading whether an AI coding agent succeeded at a task.\n");
        let _ = writeln!(prompt, "Task given to the agent:\n{}\n", self.report.prompt());
        let _ = writeln!(prompt, "Agent transcript:");
        prompt.push_str(&format_transcript(self.report.messages()));

        if let Some(diff) = self.report.agent_diff() {
            let _ = writeln!(prompt, "\nAgent's workspace diff:\n{}", truncate_chars(&diff.diff, JUDGE_CONTEXT_CHARS));
        }
        self.push_final_file_contents(&mut prompt);

        if let Some(instructions) = &self.spec.instructions {
            let _ = writeln!(prompt, "\nAdditional grading instructions:\n{instructions}\n");
        }

        let _ = writeln!(prompt, "\nRubric criteria:");
        for criterion in &self.spec.criteria {
            let _ = writeln!(
                prompt,
                "- id: {}\n  blocking: {}\n  weight: {}\n  threshold: {}\n  description: {}",
                criterion.id, criterion.blocking, criterion.weight, criterion.threshold, criterion.description
            );
        }

        let _ = writeln!(
            prompt,
            "\nReturn exactly one result for every criterion ID above and no extra criteria. Scores must be normalized numbers from 0.0 to 1.0. Do not compute weights, thresholds, blocker status, or the final score."
        );
        let _ = writeln!(prompt, "Respond with ONLY a JSON object matching this schema:");
        prompt.push_str(&judge_response_schema());
        prompt
    }

    fn summarize_response(
        &self,
        response: JudgeRubricResponse,
        raw_response: &str,
    ) -> Result<JudgeSummary, JudgeError> {
        let mut responses = BTreeMap::new();
        for criterion in response.criteria {
            let id = criterion.id.clone();
            if responses.insert(id.clone(), criterion).is_some() {
                return Err(invalid_judgment(format!("duplicate response criterion id `{id}`"), raw_response));
            }
        }

        let mut summaries = Vec::with_capacity(self.spec.criteria.len());
        let mut weighted_score = 0.0;
        let mut total_weight = 0.0;
        let mut blocking_failed = false;

        for criterion in &self.spec.criteria {
            let Some(response) = responses.remove(&criterion.id) else {
                return Err(invalid_judgment(format!("missing response criterion `{}`", criterion.id), raw_response));
            };
            if !response.score.is_finite() || !(0.0..=1.0).contains(&response.score) {
                return Err(invalid_judgment(
                    format!("criterion `{}` score must be between 0.0 and 1.0", criterion.id),
                    raw_response,
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
                reason: response.reason,
            });
        }

        if let Some(id) = responses.keys().next() {
            return Err(invalid_judgment(format!("unknown response criterion `{id}`"), raw_response));
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

    fn push_final_file_contents(&self, prompt: &mut String) {
        let paths: BTreeSet<&String> = self
            .expect
            .files
            .keys()
            .chain(self.expect.files_contain.keys())
            .chain(self.spec.context_files.iter())
            .collect();
        if paths.is_empty() {
            return;
        }

        let _ = writeln!(prompt, "\nFinal contents of files under evaluation:");
        for path in paths {
            match std::fs::read_to_string(self.report.path(path)) {
                Ok(contents) => {
                    let _ = writeln!(prompt, "--- {path} ---\n{}", truncate_chars(&contents, JUDGE_CONTEXT_CHARS));
                }
                Err(error) => {
                    let _ = writeln!(prompt, "--- {path} --- (could not read: {error})");
                }
            }
        }
    }
}

/// JSON shape the judge model is asked to respond with.
#[derive(Debug, Deserialize, JsonSchema)]
struct JudgeRubricResponse {
    criteria: Vec<JudgeCriterionResponse>,
    overall_reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JudgeCriterionResponse {
    id: String,
    score: f64,
    reason: String,
}

/// Pull the JSON object out of a judge response, tolerating prose or markdown fences the
/// model may add around it. Falls back to the trimmed input when no braces are present.
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
    serde_json::to_string_pretty(&schemars::schema_for!(JudgeRubricResponse)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffStats, GitDiff, Workspace};
    use aether_core::events::AgentMessage;
    use llm::testing::FakeLlmProvider;
    use llm::{ChatMessage, LlmError, LlmResponse, ToolCallRequest};

    const VALID_RESPONSE: &str = r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"correct"},{"id":"clarity","score":0.5,"reason":"brief"}],"overall_reason":"good"}"#;

    #[tokio::test]
    async fn judge_scores_weighted_rubric() {
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        let summary = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap();

        assert!(summary.passed);
        assert!((summary.score - 0.875).abs() < f64::EPSILON);
        assert!((summary.criteria[1].score - 0.5).abs() < f64::EPSILON);
        assert!(summary.reason.contains("all blockers met"));
    }

    #[tokio::test]
    async fn judge_zeroes_score_when_blocker_fails() {
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(
            r#"{"criteria":[{"id":"behavior","score":0.75,"reason":"wrong behavior"},{"id":"clarity","score":1.0,"reason":"clear"}],"overall_reason":"bad"}"#,
        )]);

        let summary = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap();

        assert!(!summary.passed);
        assert!(summary.score.abs() < f64::EPSILON);
        assert!(!summary.criteria[0].passed());
        assert!(summary.reason.contains("one or more blockers failed"));
    }

    #[tokio::test]
    async fn judge_rejects_invalid_criterion_sets() {
        for response in [
            r#"{"criteria":[],"overall_reason":"missing"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"ok"},{"id":"behavior","score":1.0,"reason":"ok"}],"overall_reason":"duplicate"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.0,"reason":"ok"},{"id":"clarity","score":1.0,"reason":"ok"},{"id":"extra","score":1.0,"reason":"ok"}],"overall_reason":"unknown"}"#,
            r#"{"criteria":[{"id":"behavior","score":1.5,"reason":"bad"},{"id":"clarity","score":1.0,"reason":"ok"}],"overall_reason":"score"}"#,
        ] {
            let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(response)]);

            let error = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap_err();

            assert!(matches!(error, JudgeError::InvalidJudgment { .. }), "response: {response}");
        }
    }

    #[tokio::test]
    async fn judge_extracts_json_object_from_surrounding_prose() {
        let response = format!("Here is my assessment:\n{VALID_RESPONSE}");
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(&response)]);

        let summary = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap();

        assert!(summary.passed);
    }

    #[tokio::test]
    async fn judge_returns_invalid_json_error_with_raw_response() {
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text("not json")]);

        let error = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap_err();

        let JudgeError::InvalidJson { raw_response, .. } = error else {
            panic!("expected InvalidJson, got {error:?}");
        };
        assert_eq!(raw_response, "not json");
    }

    #[tokio::test]
    async fn judge_returns_stream_error_on_llm_failure() {
        let judge_llm = FakeLlmProvider::from_results(vec![vec![Err(LlmError::Other("boom".to_string()))]]);

        let error = Judge::new(&judge_llm, &report(), &Expect::default(), &judge_spec()).run().await.unwrap_err();

        assert!(matches!(error, JudgeError::Stream(_)));
        assert!(error.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn judge_prompt_includes_rubric_task_transcript_and_json_schema() {
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
        let report = EvalReport::new("edit the file".to_string(), Workspace::empty().unwrap(), messages, None, None);
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        Judge::new(&judge_llm, &report, &Expect::default(), &judge_spec()).run().await.unwrap();

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
    async fn judge_prompt_includes_stored_agent_diff() {
        let diff = GitDiff {
            diff: "diff --git a/a.txt b/a.txt\n+new\n".to_string(),
            stats: DiffStats { files_changed: 1, lines_added: 1, lines_removed: 0 },
        };
        let report =
            EvalReport::new("p".to_string(), Workspace::empty().unwrap(), vec![AgentMessage::Done], Some(diff), None);
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        Judge::new(&judge_llm, &report, &Expect::default(), &judge_spec()).run().await.unwrap();

        let prompt = judged_prompt(&judge_llm);
        assert!(prompt.contains("Agent's workspace diff"));
        assert!(prompt.contains("+new"));
    }

    #[tokio::test]
    async fn judge_prompt_includes_final_contents_of_files_under_evaluation() {
        let workspace =
            Workspace::from_files([("notes.txt", "beta\nalpha\n"), ("extra.txt", "extra context")]).unwrap();
        let report = EvalReport::new("p".to_string(), workspace, vec![AgentMessage::Done], None, None);
        let expect =
            Expect { files_contain: [("notes.txt".to_string(), "beta".to_string())].into(), ..Expect::default() };
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(VALID_RESPONSE)]);

        Judge::new(&judge_llm, &report, &expect, &judge_spec()).run().await.unwrap();

        let prompt = judged_prompt(&judge_llm);
        assert!(prompt.contains("--- notes.txt ---"));
        assert!(prompt.contains("beta\nalpha"));
        assert!(prompt.contains("--- extra.txt ---"));
        assert!(prompt.contains("extra context"));
    }

    fn report() -> EvalReport {
        EvalReport::new("prompt".to_string(), Workspace::empty().unwrap(), vec![AgentMessage::Done], None, None)
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
