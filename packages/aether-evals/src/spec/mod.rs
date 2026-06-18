mod error;
mod judge;
mod report;
mod runner;
mod types;

pub use error::EvalFileError;
pub use judge::{Judge, JudgeBuilder, JudgeContext, JudgeCriterionResponse, JudgeError, JudgeRubricResponse, judge};
pub use report::{EvalFilesReport, EvalOutcome, EvalStreamEvent, EvalToolCall, JudgeCriterionSummary, JudgeSummary};
pub use runner::{WorkspaceRetention, run_eval_spec_streaming, run_eval_specs};
use serde_json::from_str;
pub(crate) use types::ResolvedDocker;
pub use types::{
    AgentSpec, DockerSpec, Expect, JudgeCriterionSpec, JudgeRef, JudgeSpec, TaskSpec, ToolCallExpectation,
    WorkspaceSpec,
};

use schemars::{JsonSchema, Schema, schema_for};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::read_to_string,
    path::{Path, PathBuf},
};

use crate::{CONTAINER_AETHER_HOME, DockerAgent, default_eval_env_vars};

const DEFAULT_EVALS_DIR: &str = "evals";

/// A JSON serializable representation of a single eval.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalSpec {
    pub name: String,
    /// Docker configuration (image, Dockerfile, build context).
    pub docker: DockerSpec,

    /// Command that emits newline-delimited `AgentMessage` JSON on stdout.
    pub agent: AgentSpec,

    pub task: TaskSpec,
    #[serde(default)]
    pub expect: Expect,
}

impl EvalSpec {
    /// JSON schema for the eval spec (the input authored as JSON / generated as a TypeScript type).
    pub fn schema() -> Schema {
        schema_for!(Self)
    }
}

/// Options for discovering and loading resolved eval specs.
#[derive(Debug, Clone, Default)]
pub struct EvalSpecLoadOptions {
    /// Eval files or directories to discover eval files under. Empty means `./evals`.
    pub paths: Vec<PathBuf>,
    /// When set, load only the eval with this name.
    pub filter: Option<String>,
}

/// An [`EvalSpec`] resolved against its base directory.
#[derive(Debug)]
pub struct ResolvedEvalSpec {
    pub name: String,
    pub base_dir: PathBuf,
    pub(crate) docker: ResolvedDocker,
    pub(crate) agent: AgentSpec,
    pub(crate) task: TaskSpec,
    pub(crate) expect: Expect,
    pub(crate) judge: Option<JudgeSpec>,
}

impl ResolvedEvalSpec {
    /// Discover, read, and parse eval files using the provided options. Directory discovery is
    /// recursive and includes files ending in `.eval.json`. Explicit file paths are always treated
    /// as eval files, regardless of filename.
    pub fn load(options: EvalSpecLoadOptions) -> Result<Vec<Self>, EvalFileError> {
        let paths = if options.paths.is_empty() { vec![PathBuf::from(DEFAULT_EVALS_DIR)] } else { options.paths };
        let mut spec_paths = Vec::new();
        for path in &paths {
            discover_specs_at(path, &mut spec_paths)?;
        }

        spec_paths.sort();
        spec_paths.dedup();
        let mut cases = spec_paths.into_iter().map(Self::load_file).collect::<Result<Vec<_>, _>>()?;
        if cases.is_empty() {
            return Err(EvalFileError::NoEvalFilesFound { paths });
        }

        reject_duplicate_names(&cases)?;

        if let Some(name) = &options.filter {
            cases.retain(|case| &case.name == name);
            if cases.is_empty() {
                return Err(EvalFileError::NoMatchingEval { name: name.clone() });
            }
        }

        Ok(cases)
    }

    /// Read and parse an eval file, resolving it against its base directory (the eval file's
    /// parent).
    pub fn load_file(path: impl AsRef<Path>) -> Result<Self, EvalFileError> {
        let path = path.as_ref();
        let path =
            path.canonicalize().map_err(|source| EvalFileError::ReadEvalFile { path: path.to_path_buf(), source })?;

        let content =
            read_to_string(&path).map_err(|source| EvalFileError::ReadEvalFile { path: path.clone(), source })?;

        let eval: EvalSpec =
            from_str(&content).map_err(|source| EvalFileError::ParseEvalFile { path: path.clone(), source })?;

        let base_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        Self::resolve(eval, base_dir)
    }

    /// Resolve a parsed eval file against `base_dir`, resolving the Docker image and reading
    /// referenced judge files.
    pub fn resolve(eval: EvalSpec, base_dir: impl Into<PathBuf>) -> Result<Self, EvalFileError> {
        let EvalSpec { name, docker, agent, task, expect } = eval;
        let base_dir = base_dir.into();
        agent.validate().map_err(|message| EvalFileError::InvalidAgentCommand { message })?;

        let docker = docker.resolve(&base_dir)?;
        let judge = expect.judge.as_ref().map(|judge| judge.resolve(&base_dir)).transpose()?;

        Ok(Self { name, base_dir, docker, agent, task, expect, judge })
    }
}

impl ResolvedEvalSpec {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn task(&self) -> Result<crate::Task, crate::WorkspaceError> {
        self.task.build(&self.base_dir)
    }

    pub(crate) fn agent(&self) -> DockerAgent {
        DockerAgent::new(self.docker.image.clone(), self.agent.command.clone())
            .with_env_vars(default_eval_env_vars())
            .with_env_vars(self.agent.env.clone())
            .with_ephemeral_mount(CONTAINER_AETHER_HOME)
    }

    pub(crate) fn expectations(&self) -> &Expect {
        &self.expect
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

fn reject_duplicate_names(cases: &[ResolvedEvalSpec]) -> Result<(), EvalFileError> {
    let mut names = BTreeSet::new();
    for case in cases {
        if !names.insert(case.name.as_str()) {
            return Err(EvalFileError::DuplicateEvalName { name: case.name.clone() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_camel_case_spec_with_inline_files() {
        let spec: EvalSpec = serde_json::from_str(
            r#"{"docker":{"file":"Dockerfile","image":"sandbox:latest","context":"../../.."},"name":"edit","agent":{"command":["agent"]},"task":{"prompt":"do it","workspace":{"files":{"a.txt":"hi"}}},"expect":{"toolCalls":{"coding__read_file":{"atLeast":1},"coding__edit_file":{"exactly":1}},"files":{"a.txt":"bye"},"judge":{"model":"anthropic:claude-sonnet-4-5","instructions":"grade it","contextFiles":["a.txt"],"criteria":[{"id":"behavior","description":"did it work?"},{"id":"clarity","description":"explained it","blocking":false,"weight":0.5,"threshold":0.7}]}}}"#,
        )
        .unwrap();

        assert!(matches!(
            &spec.docker,
            DockerSpec::Build { file, image: Some(image), context: Some(context) }
                if file == Path::new("Dockerfile") && image == "sandbox:latest" && context == Path::new("../../..")
        ));
        assert_eq!(spec.name, "edit");
        assert!(matches!(&spec.task.workspace, WorkspaceSpec::Files(files) if files["a.txt"] == "hi"));
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
            r#"{"docker":{"image":"sandbox:latest"},"name":"g","agent":{"command":["agent"]},"task":{"prompt":"p","workspace":{"git":{"url":"u","startCommit":"a","goldCommit":"b"}}}}"#,
        )
        .unwrap();
        assert!(matches!(git.task.workspace, WorkspaceSpec::Git(_)));

        let dir: EvalSpec = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"d","agent":{"command":["agent"]},"task":{"prompt":"p","workspace":{"dir":"fixtures/x"}}}"#,
        )
        .unwrap();
        assert!(matches!(dir.task.workspace, WorkspaceSpec::Dir(_)));
    }

    #[test]
    fn rejects_invalid_docker_specs() {
        for (json, expected) in [
            (r#"{"docker":{},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#, "docker must specify"),
            (
                r#"{"docker":{"image":"sandbox:latest","context":"x"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#,
                "`context` requires a Dockerfile `file`",
            ),
            (
                r#"{"docker":{"dockerfile":"Dockerfile"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#,
                "unknown field `dockerfile`",
            ),
        ] {
            let error = serde_json::from_str::<EvalSpec>(json).unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected:?} in {error:?}");
        }
    }

    #[test]
    fn rejects_invalid_tool_call_expectations() {
        for json in [
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"toolCalls":{"bash":{}}}}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"toolCalls":{"bash":{"atLeast":1,"exactly":1}}}}"#,
        ] {
            assert!(serde_json::from_str::<EvalSpec>(json).is_err(), "expected rejection for {json}");
        }

        let unknown = serde_json::from_str::<EvalSpec>(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"toolCalls":{"bash":{"minimum":1}}}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(unknown.contains("unknown variant `minimum`"), "got: {unknown}");
    }

    #[test]
    fn rejects_judge_without_model() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"criteria":[{"id":"ok","description":"ok"}]}}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("missing field `model`"));
    }

    #[test]
    fn rejects_unknown_fields() {
        for json in [
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"bogus":true}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"runtime":{"type":"acp","command":["a"]}}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"baseDir":"."}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"judge":null}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"type":"aether","command":["nope"]},"task":{"prompt":"p"}}"#,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["node"],"modelName":"m"},"task":{"prompt":"p"}}"#,
        ] {
            let result: Result<EvalSpec, _> = serde_json::from_str(json);
            assert!(result.is_err());
        }
    }

    #[test]
    fn rejects_workspace_with_more_than_one_source() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p","workspace":{"files":{},"dir":"x"}}}"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_workspace_with_unknown_field_by_name() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p","workspace":{"dirr":"x"}}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("unknown variant `dirr`"));
    }

    #[test]
    fn rejects_git_workspace_with_unknown_field_by_name() {
        let result: Result<EvalSpec, _> = serde_json::from_str(
            r#"{"docker":{"image":"sandbox:latest"},"name":"g","agent":{"command":["agent"]},"task":{"prompt":"p","workspace":{"git":{"url":"u","startCommit":"a","goldComit":"b"}}}}"#,
        );

        assert!(result.unwrap_err().to_string().contains("unknown field `goldComit`"));
    }

    #[test]
    fn rejects_invalid_judge_criteria() {
        for (json, expected) in [
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[]}}}"#,
                "criteria must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[{"id":"","description":"ok"}]}}}"#,
                "id must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok"},{"id":"a","description":"ok"}]}}}"#,
                "duplicate judge criterion id `a`",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[{"id":"a","description":""}]}}}"#,
                "description must not be empty",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok","weight":0.0}]}}}"#,
                "weight must be positive and finite",
            ),
            (
                r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"m","criteria":[{"id":"a","description":"ok","threshold":1.5}]}}}"#,
                "threshold must be between 0.0 and 1.0",
            ),
        ] {
            let eval = serde_json::from_str::<EvalSpec>(json).unwrap();
            let error = ResolvedEvalSpec::resolve(eval, ".").unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected:?} in {error:?}");
        }
    }
}
