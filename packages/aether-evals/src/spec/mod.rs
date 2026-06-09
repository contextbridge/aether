mod error;
mod judge;
mod report;
mod runner;
mod types;

pub use error::EvalFileError;
pub use report::{EvalFilesReport, EvalOutcome, JudgeCriterionSummary, JudgeSummary};
pub use runner::{EvalRunOptions, run_eval_files};
pub(crate) use types::{DockerSpec, Expect, JudgeSpec, ResolvedDocker, SettingsRef, WorkspaceSpec};
#[cfg(test)]
use types::{JudgeRef, ToolCallExpectation};

use self::judge::{Judge, JudgeError};
use crate::agents::{Agent, DockerAetherAgent};
use crate::{EvalRunError, WorkspaceError, run_eval};
use aether_project::AetherSettings;
use llm::StreamingModelProvider;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// An [`EvalSpec`] resolved against its base directory (the eval file's parent), against which
/// relative paths resolve. Docker, judge, and settings references are resolved once at
/// construction, so a broken reference fails at load time rather than after the agent run.
#[derive(Debug)]
pub(crate) struct EvalCase {
    pub eval: EvalSpec,
    pub base_dir: PathBuf,
    pub(crate) docker: Option<ResolvedDocker>,
    pub(crate) judge: Option<JudgeSpec>,
    pub(crate) settings: Option<AetherSettings>,
}

/// A declarative eval file, authored as JSON, that describes one scenario to run against a
/// Dockerized `aether headless` agent.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvalSpec {
    /// Docker configuration (image, Dockerfile, build context).
    #[serde(default)]
    pub docker: Option<DockerSpec>,

    /// Named agent from settings to pass through to `aether headless --agent`.
    #[serde(default)]
    pub agent: Option<String>,

    /// Aether settings to drive the agent: either a path (relative to the eval file) or an inline
    /// settings object.
    #[serde(default)]
    pub settings: Option<SettingsRef>,

    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub workspace: Option<WorkspaceSpec>,
    #[serde(default)]
    pub expect: Expect,
}

impl EvalSpec {
    /// Read and parse an eval file, resolving it against its base directory (the eval file's
    /// parent).
    pub(crate) fn load(path: impl AsRef<Path>) -> Result<EvalCase, EvalFileError> {
        let path = path.as_ref();
        let path =
            path.canonicalize().map_err(|source| EvalFileError::ReadEvalFile { path: path.to_path_buf(), source })?;
        let content = std::fs::read_to_string(&path)
            .map_err(|source| EvalFileError::ReadEvalFile { path: path.clone(), source })?;
        let eval: EvalSpec = serde_json::from_str(&content)
            .map_err(|source| EvalFileError::ParseEvalFile { path: path.clone(), source })?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        EvalCase::new(eval, base_dir)
    }

    /// Discover, read, and parse eval files under the provided files or directories. Directory
    /// discovery is recursive and includes files ending in `.eval.json`. Explicit file paths are
    /// always treated as eval files, regardless of filename.
    pub(crate) fn load_all(paths: &[PathBuf]) -> Result<Vec<EvalCase>, EvalFileError> {
        let mut spec_paths = Vec::new();
        for path in paths {
            discover_specs_at(path, &mut spec_paths)?;
        }

        spec_paths.sort();
        spec_paths.dedup();
        spec_paths.into_iter().map(Self::load).collect()
    }
}

impl EvalCase {
    /// Resolve a parsed eval file against `base_dir`, resolving the Docker image and reading
    /// referenced judge and settings files.
    pub(crate) fn new(eval: EvalSpec, base_dir: impl Into<PathBuf>) -> Result<Self, EvalFileError> {
        let base_dir = base_dir.into();
        let docker = eval.docker.as_ref().map(|docker| docker.resolve(&base_dir)).transpose()?;
        let judge = eval.expect.judge.as_ref().map(|judge| judge.resolve(&base_dir)).transpose()?;
        let settings = eval.settings.as_ref().map(|settings| settings.resolve(&base_dir)).transpose()?;
        Ok(Self { eval, base_dir, docker, judge, settings })
    }

    /// Assemble the Dockerized agent for this case from its resolved image, settings, and agent
    /// name. Fails when the eval file has no `docker` section.
    pub(crate) fn agent(&self) -> Result<DockerAetherAgent, EvalFileError> {
        let docker = self.docker.as_ref().ok_or(EvalFileError::NoImage)?;
        let mut agent = DockerAetherAgent::new(docker.image.clone());
        if let Some(settings) = &self.settings {
            agent = agent.with_settings(settings.clone());
        }

        if let Some(agent_name) = self.eval.agent.as_deref() {
            agent = agent.with_agent(agent_name);
        }

        Ok(agent)
    }

    /// Run this eval against an already-constructed agent. `judge_llm` is the parsed judge model
    /// when the case has a judge. Faults — workspace setup, the agent run, the judge call — are
    /// recorded as failures on the returned outcome rather than surfaced as errors.
    pub(crate) async fn run_with(
        &self,
        agent: &impl Agent,
        judge_llm: Option<&dyn StreamingModelProvider>,
    ) -> EvalOutcome {
        match self.run_checks(agent, judge_llm).await {
            Ok(outcome) => outcome,
            Err(error) => EvalOutcome {
                name: self.eval.name.clone(),
                passed: false,
                failures: vec![error.to_string()],
                judge: None,
                failure_context: None,
            },
        }
    }

    async fn run_checks(
        &self,
        agent: &impl Agent,
        judge_llm: Option<&dyn StreamingModelProvider>,
    ) -> Result<EvalOutcome, EvalRunFailure> {
        let workspace = match &self.eval.workspace {
            Some(spec) => spec.build(&self.base_dir)?,
            None => crate::Workspace::empty()?,
        };
        let report = run_eval(agent, &self.eval.prompt, workspace).await?;
        let mut failures = self.eval.expect.evaluate(&report);
        let judge = match &self.judge {
            Some(spec) => {
                let llm = judge_llm.expect("judge model is parsed before any eval runs when a judge is configured");
                let summary = Judge::new(llm, &report, &self.eval.expect, spec).run().await?;
                failures.extend(summary.blocking_failures());
                Some(summary)
            }
            None => None,
        };

        let passed = failures.is_empty();
        let failure_context = (!passed).then(|| report.failure_context());

        Ok(EvalOutcome { name: self.eval.name.clone(), passed, failures, judge, failure_context })
    }
}

fn discover_specs_at(path: &Path, spec_paths: &mut Vec<PathBuf>) -> Result<(), EvalFileError> {
    if path.is_file() {
        spec_paths.push(path.to_path_buf());
        return Ok(());
    }

    if !path.is_dir() {
        return Err(EvalFileError::EvalPathNotFound { path: path.to_path_buf() });
    }

    let mut entries = std::fs::read_dir(path)
        .map_err(|source| EvalFileError::DiscoverEvalFiles { path: path.to_path_buf(), source })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| EvalFileError::DiscoverEvalFiles { path: path.to_path_buf(), source })?;

    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            discover_specs_at(&path, spec_paths)?;
        } else if is_discovered_spec(&path) {
            spec_paths.push(path);
        }
    }

    Ok(())
}

fn is_discovered_spec(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.ends_with(".eval.json")
}

#[derive(Debug, Error)]
enum EvalRunFailure {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] WorkspaceError),

    #[error("agent run failed: {0}")]
    Run(#[from] EvalRunError),

    #[error("judge failed: {0}")]
    Judge(#[from] JudgeError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_camel_case_spec_with_inline_files() {
        let spec: EvalSpec = serde_json::from_str(
            r#"{
                "docker": { "file": "Dockerfile", "image": "sandbox:latest", "context": "../../.." },
                "name": "edit",
                "prompt": "do it",
                "workspace": { "files": { "a.txt": "hi" } },
                "expect": {
                    "toolCalls": {
                        "coding__read_file": { "atLeast": 1 },
                        "coding__edit_file": { "exactly": 1 }
                    },
                    "files": { "a.txt": "bye" },
                    "judge": {
                        "model": "anthropic:claude-sonnet-4-5",
                        "instructions": "grade it",
                        "contextFiles": ["a.txt"],
                        "criteria": [
                            { "id": "behavior", "description": "did it work?" },
                            { "id": "clarity", "description": "explained it", "blocking": false, "weight": 0.5, "threshold": 0.7 }
                        ]
                    }
                }
            }"#,
        )
        .unwrap();

        assert!(matches!(
            &spec.docker,
            Some(DockerSpec::Build { file, image: Some(image), context: Some(context) })
                if file == Path::new("Dockerfile") && image == "sandbox:latest" && context == Path::new("../../..")
        ));
        assert_eq!(spec.name, "edit");
        assert!(matches!(&spec.workspace, Some(WorkspaceSpec::Files(files)) if files["a.txt"] == "hi"));
        assert!(matches!(spec.expect.tool_calls["coding__read_file"], ToolCallExpectation::AtLeast(1)));
        assert!(matches!(spec.expect.tool_calls["coding__edit_file"], ToolCallExpectation::Exactly(1)));
        let Some(JudgeRef::Inline(judge)) = &spec.expect.judge else {
            panic!("expected an inline judge, got {:?}", spec.expect.judge);
        };
        assert_eq!(judge.instructions.as_deref(), Some("grade it"));
        assert_eq!(judge.context_files, vec!["a.txt".to_string()]);
        assert_eq!(judge.criteria[0].id, "behavior");
        assert!(judge.criteria[0].blocking);
        assert!((judge.criteria[0].weight - 1.0).abs() < f64::EPSILON);
        assert!((judge.criteria[0].threshold - 1.0).abs() < f64::EPSILON);
        assert!(!judge.criteria[1].blocking);
        assert!((judge.criteria[1].weight - 0.5).abs() < f64::EPSILON);
        assert!((judge.criteria[1].threshold - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_git_and_dir_workspaces() {
        let git: EvalSpec = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"g","prompt":"p","workspace":{"git":{"url":"u","startCommit":"a","goldCommit":"b"}}}"#,
        )
        .unwrap();
        assert!(matches!(git.workspace, Some(WorkspaceSpec::Git(_))));

        let dir: EvalSpec = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"d","prompt":"p","workspace":{"dir":"fixtures/x"}}"#,
        )
        .unwrap();
        assert!(matches!(dir.workspace, Some(WorkspaceSpec::Dir(_))));
    }

    #[test]
    fn rejects_invalid_docker_specs() {
        for (json, expected) in [
            (r#"{"docker":{},"name":"c","prompt":"p"}"#, "docker must specify"),
            (
                r#"{"docker":{"image":"sandbox:latest","context":"x"},"name":"c","prompt":"p"}"#,
                "`context` requires a Dockerfile `file`",
            ),
            (r#"{"docker":{"dockerfile":"Dockerfile"},"name":"c","prompt":"p"}"#, "unknown field `dockerfile`"),
        ] {
            let error = serde_json::from_str::<EvalSpec>(json).unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected:?} in {error:?}");
        }
    }

    #[test]
    fn rejects_invalid_tool_call_expectations() {
        for (json, expected) in [
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"toolCalls":{"bash":{}}}}"#,
                "exactly one of `atLeast` or `exactly`",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"toolCalls":{"bash":{"atLeast":1,"exactly":1}}}}"#,
                "exactly one of `atLeast` or `exactly`",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"toolCalls":{"bash":{"minimum":1}}}}"#,
                "unknown field `minimum`",
            ),
        ] {
            let error = serde_json::from_str::<EvalSpec>(json).unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected:?} in {error:?}");
        }
    }

    #[test]
    fn rejects_judge_without_model() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"criteria":[{"id":"ok","description":"ok"}]}}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("missing field `model`"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<EvalSpec, _> =
            serde_json::from_str(r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","bogus":true}"#);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_workspace_with_more_than_one_source() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","workspace":{"files":{},"dir":"x"}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("exactly one of"));
    }

    #[test]
    fn rejects_workspace_with_unknown_field_by_name() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","workspace":{"dirr":"x"}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("unknown field `dirr`"));
    }

    #[test]
    fn rejects_git_workspace_with_unknown_field_by_name() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"g","prompt":"p","workspace":{"git":{"url":"u","startCommit":"a","goldComit":"b"}}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("unknown field `goldComit`"));
    }

    #[test]
    fn rejects_invalid_judge_criteria() {
        for (json, expected) in [
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[]}}}"#,
                "criteria must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[{"id":"","description":"ok"}]}}}"#,
                "id must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok"},{"id":"a","description":"ok"}]}}}"#,
                "duplicate judge criterion id `a`",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[{"id":"a","description":""}]}}}"#,
                "description must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok","weight":0.0}]}}}"#,
                "weight must be positive and finite",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok","threshold":1.5}]}}}"#,
                "threshold must be between 0.0 and 1.0",
            ),
        ] {
            let eval = serde_json::from_str::<EvalSpec>(json).unwrap();
            let error = EvalCase::new(eval, ".").unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected:?} in {error:?}");
        }
    }
}
