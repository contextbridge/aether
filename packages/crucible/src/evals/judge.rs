use crate::git_repo::GitRepo;
use crate::{AgentEvalMessage, Workspace, WorkspaceSource};
use thiserror::Error;

pub struct LlmJudgeContext<'a> {
    pub workspace: &'a Workspace,
    pub original_prompt: &'a str,
    pub messages: &'a [AgentEvalMessage],
}

#[derive(Debug)]
pub struct JudgeResult {
    passed: bool,
    reason: String,
    raw_response: String,
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
}

impl JudgeResult {
    pub(crate) fn new(passed: bool, reason: impl Into<String>, raw_response: impl Into<String>) -> Self {
        Self { passed, reason: reason.into(), raw_response: raw_response.into() }
    }

    pub fn passed(&self) -> bool {
        self.passed
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn raw_response(&self) -> &str {
        &self.raw_response
    }
}

impl LlmJudgeContext<'_> {
    pub fn git_diff(&self, to_commit: Option<&str>) -> Option<String> {
        match self.workspace.source() {
            WorkspaceSource::GitRepo { start_commit, .. } => {
                let git_repo = GitRepo::from_path(self.workspace.path());
                match to_commit {
                    Some(commit) => git_repo.diff(start_commit, commit).ok(),
                    None => git_repo.diff_unstaged().ok(),
                }
            }
            WorkspaceSource::Local => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evals::report::EvalReport;
    use llm::testing::FakeLlmProvider;
    use llm::{LlmError, LlmResponse};

    #[tokio::test]
    async fn judge_passes_on_successful_binary_metric() {
        let report = report();
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(
            r#"{"type":"binary","success":true,"reason":"looks good"}"#,
        )]);

        let judgment = report.judge(&judge_llm, |_| "judge this".to_string()).await.unwrap();

        assert!(judgment.passed());
        assert_eq!(judgment.reason(), "looks good");
    }

    #[tokio::test]
    async fn judge_fails_on_unsuccessful_binary_metric() {
        let report = report();
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text(
            r#"{"type":"binary","success":false,"reason":"not good"}"#,
        )]);

        let judgment = report.judge(&judge_llm, |_| "judge this".to_string()).await.unwrap();

        assert!(!judgment.passed());
        assert_eq!(judgment.reason(), "not good");
    }

    #[tokio::test]
    async fn judge_returns_invalid_json_error_with_raw_response() {
        let report = report();
        let judge_llm = FakeLlmProvider::with_single_response(vec![LlmResponse::text("not json")]);

        let error = report.judge(&judge_llm, |_| "judge this".to_string()).await.unwrap_err();

        let JudgeError::InvalidJson { raw_response, .. } = error else {
            panic!("expected InvalidJson, got {error:?}");
        };
        assert_eq!(raw_response, "not json");
    }

    #[tokio::test]
    async fn judge_returns_stream_error_on_llm_failure() {
        let report = report();
        let judge_llm = FakeLlmProvider::from_results(vec![vec![Err(LlmError::Other("boom".to_string()))]]);

        let error = report.judge(&judge_llm, |_| "judge this".to_string()).await.unwrap_err();

        assert!(matches!(error, JudgeError::Stream(_)));
        assert!(error.to_string().contains("boom"));
    }

    fn report() -> EvalReport {
        EvalReport::new("prompt".to_string(), Workspace::empty().unwrap(), vec![AgentEvalMessage::Done], None, None)
    }
}
