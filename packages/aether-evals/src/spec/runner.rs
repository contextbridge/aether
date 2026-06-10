use super::error::EvalFileError;
use super::report::EvalFilesReport;
use super::{EvalCase, EvalSpec};
use crate::agents::build_images;
use futures::StreamExt;
use llm::parser::ModelProviderParser;
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;

const DEFAULT_EVALS_DIR: &str = "evals";

/// Request to discover and run a collection of declarative eval files.
#[derive(Debug, Clone)]
pub struct EvalRunOptions {
    /// Eval files or directories to discover eval files under. Empty means `./evals`.
    pub paths: Vec<PathBuf>,
    /// When set, run only the eval with this name.
    pub filter: Option<String>,
    pub max_concurrency: NonZeroUsize,
}

/// Discover, validate, and run declarative eval files. Agents and judge models are assembled
/// before anything runs, so a broken case fails fast. Docker image builds are deduplicated and
/// run concurrently before eval execution. Outcomes are returned in discovery order.
pub async fn run_eval_files(options: EvalRunOptions) -> Result<EvalFilesReport, EvalFileError> {
    let paths = if options.paths.is_empty() { vec![PathBuf::from(DEFAULT_EVALS_DIR)] } else { options.paths };
    let mut cases = EvalSpec::load_all(&paths)?;
    if cases.is_empty() {
        return Err(EvalFileError::NoEvalFilesFound { paths });
    }

    reject_duplicate_names(&cases)?;

    if let Some(name) = &options.filter {
        cases.retain(|case| &case.eval.name == name);
        if cases.is_empty() {
            return Err(EvalFileError::NoMatchingEval { name: name.clone() });
        }
    }

    let parser = ModelProviderParser::default();
    let mut ready = Vec::with_capacity(cases.len());
    for case in cases {
        let agent = case.agent()?;
        let judge_llm = match &case.judge {
            Some(judge) => {
                let (llm, _) = parser
                    .parse(&judge.model)
                    .await
                    .map_err(|source| EvalFileError::JudgeModel { model: judge.model.clone(), source })?;
                Some(llm)
            }
            None => None,
        };
        ready.push((case, agent, judge_llm));
    }

    let builds = ready.iter().filter_map(|(case, ..)| case.docker.as_ref().and_then(|docker| docker.build.clone()));
    build_images(builds, options.max_concurrency).await?;

    let evals = futures::stream::iter(
        ready
            .into_iter()
            .map(|(case, agent, judge_llm)| async move { case.run_with(&agent, judge_llm.as_deref()).await }),
    )
    .buffered(options.max_concurrency.get())
    .collect()
    .await;

    Ok(EvalFilesReport { evals })
}

fn reject_duplicate_names(cases: &[EvalCase]) -> Result<(), EvalFileError> {
    let mut names = BTreeSet::new();
    for case in cases {
        if !names.insert(case.eval.name.as_str()) {
            return Err(EvalFileError::DuplicateEvalName { name: case.eval.name.clone() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeAgent;
    use crate::agents::{DockerImage, ImageBuildError};
    use crate::spec::{EvalCase, EvalFileError, SettingsRef};
    use std::path::{Path, PathBuf};

    fn eval_case(json: &str) -> EvalCase {
        EvalCase::new(serde_json::from_str(json).unwrap(), ".").unwrap()
    }

    fn eval_case_in(json: &str, base_dir: &Path) -> EvalCase {
        EvalCase::new(serde_json::from_str(json).unwrap(), base_dir).unwrap()
    }

    #[tokio::test]
    async fn run_with_passes_when_all_expectations_met() {
        let case = eval_case(
            r#"{
                "name": "expectations",
                "prompt": "do the thing",
                "workspace": { "files": { "notes.txt": "beta\n" } },
                "expect": {
                    "toolCalls": { "bash": { "exactly": 1 } },
                    "files": { "notes.txt": "beta\n" },
                    "filesContain": { "notes.txt": "beta" }
                }
            }"#,
        );

        let outcome = case.run_with(&FakeAgent::with_tool_call("bash", "ok"), None).await;

        assert!(outcome.passed, "failures: {:?}", outcome.failures);
    }

    #[tokio::test]
    async fn run_with_reports_missing_tool_and_wrong_count() {
        let case = eval_case(
            r#"{
                "name": "counts",
                "prompt": "p",
                "expect": { "toolCalls": { "read": { "atLeast": 1 }, "bash": { "exactly": 2 } } }
            }"#,
        );

        let outcome = case.run_with(&FakeAgent::with_tool_call("bash", "ok"), None).await;

        assert!(!outcome.passed);
        assert!(outcome.failures.iter().any(|f| f.contains("`read`")));
        assert!(outcome.failures.iter().any(|f| f.contains("`bash`") && f.contains("2 time")));
    }

    #[tokio::test]
    async fn run_with_reports_file_mismatch_and_missing_file() {
        let case = eval_case(
            r#"{
                "name": "files",
                "prompt": "p",
                "workspace": { "files": { "notes.txt": "alpha\n" } },
                "expect": { "files": { "notes.txt": "beta\n" }, "filesContain": { "missing.txt": "x" } }
            }"#,
        );

        let outcome = case.run_with(&FakeAgent::with_tool_call("bash", "ok"), None).await;

        assert!(!outcome.passed);
        assert!(outcome.failures.iter().any(|f| f.contains("`notes.txt`") && f.contains("mismatch")));
        assert!(outcome.failures.iter().any(|f| f.contains("`missing.txt`") && f.contains("could not be read")));
    }

    #[tokio::test]
    async fn run_with_records_fault_as_failed_eval_instead_of_aborting() {
        let base_dir = tempfile::tempdir().unwrap();
        let case = eval_case_in(r#"{"name":"broken","prompt":"p","workspace":{"dir":"missing-dir"}}"#, base_dir.path());

        let outcome = case.run_with(&FakeAgent::with_tool_call("bash", "ok"), None).await;

        assert!(!outcome.passed);
        assert!(outcome.failures[0].contains("workspace setup failed"), "got: {:?}", outcome.failures);
    }

    #[test]
    fn agent_rejects_case_without_docker_section() {
        let case = eval_case(r#"{"name":"c","prompt":"p"}"#);

        let error = case.agent().map(|_| ()).unwrap_err();

        assert!(matches!(error, EvalFileError::NoImage));
    }

    #[test]
    fn load_rejects_missing_dockerfile() {
        let base_dir = tempfile::tempdir().unwrap();
        let eval = serde_json::from_str(r#"{"docker":{"file":"Dockerfile"},"name":"x","prompt":"p"}"#).unwrap();

        let error = EvalCase::new(eval, base_dir.path()).unwrap_err();

        assert!(matches!(error, EvalFileError::DockerPath { .. }));
    }

    #[tokio::test]
    async fn run_evals_rejects_conflicting_image_tags_before_running() {
        let dir = tempfile::tempdir().unwrap();
        for (subdir, name) in [("a", "one"), ("b", "two")] {
            let eval_dir = dir.path().join(subdir);
            std::fs::create_dir_all(&eval_dir).unwrap();
            std::fs::write(eval_dir.join("Dockerfile"), "FROM scratch").unwrap();
            std::fs::write(
                eval_dir.join(format!("{name}.eval.json")),
                format!(r#"{{"docker":{{"file":"Dockerfile","image":"same:tag"}},"name":"{name}","prompt":"p"}}"#),
            )
            .unwrap();
        }
        let error = run_eval_files(EvalRunOptions {
            paths: vec![dir.path().to_path_buf()],
            filter: None,
            max_concurrency: NonZeroUsize::new(2).unwrap(),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, EvalFileError::ImageBuild(ImageBuildError::ConflictingTag { .. })));
    }

    #[tokio::test]
    async fn run_evals_rejects_invalid_judge_model_before_running() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad-judge.eval.json"),
            r#"{"docker":{"image":"sandbox:latest"},"name":"bad-judge","prompt":"p","expect":{"judge":{"model":"not-a-provider:nope","criteria":[{"id":"a","description":"ok"}]}}}"#,
        )
        .unwrap();

        let error = run_eval_files(EvalRunOptions {
            paths: vec![dir.path().to_path_buf()],
            filter: None,
            max_concurrency: NonZeroUsize::new(1).unwrap(),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, EvalFileError::JudgeModel { .. }), "got: {error:?}");
    }

    #[test]
    fn load_reads_spec_and_returns_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(&path, r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p"}"#).unwrap();

        let case = EvalSpec::load(&path).unwrap();

        let docker = case.docker.as_ref().expect("expected a resolved docker image");
        assert_eq!(docker.image, DockerImage::new("sandbox", "latest"));
        assert!(docker.build.is_none());
        assert_eq!(case.eval.name, "c");
        assert_eq!(case.base_dir, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn load_resolves_dockerfile_build_relative_to_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch").unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(&path, r#"{"docker":{"file":"Dockerfile","image":"sandbox:dev"},"name":"c","prompt":"p"}"#)
            .unwrap();

        let case = EvalSpec::load(&path).unwrap();

        let docker = case.docker.as_ref().expect("expected a resolved docker image");
        assert_eq!(docker.image, DockerImage::new("sandbox", "dev"));
        let build = docker.build.as_ref().expect("expected a build request");
        assert_eq!(build.dockerfile, dir.path().canonicalize().unwrap().join("Dockerfile"));
        assert_eq!(build.context, dir.path().canonicalize().unwrap());
        assert_eq!(build.tag, "sandbox:dev");
    }

    #[test]
    fn load_distinguishes_missing_file_from_invalid_json() {
        let missing = EvalSpec::load("/nonexistent/example.eval.json").unwrap_err();
        assert!(matches!(missing, EvalFileError::ReadEvalFile { .. }));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(EvalSpec::load(&path).unwrap_err(), EvalFileError::ParseEvalFile { .. }));
    }

    #[test]
    fn load_resolves_shared_judge_file_relative_to_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("shared")).unwrap();
        std::fs::write(
            dir.path().join("shared/maintainer.judge.json"),
            r#"{"model":"anthropic:claude-sonnet-4-5","instructions":"grade it","criteria":[{"id":"behavior","description":"works"}]}"#,
        )
        .unwrap();
        let path = dir.path().join("nested/example.eval.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":"../shared/maintainer.judge.json"}}"#,
        )
        .unwrap();

        let case = EvalSpec::load(&path).unwrap();

        let judge = case.judge.as_ref().expect("expected a resolved judge");
        assert_eq!(judge.model, "anthropic:claude-sonnet-4-5");
        assert_eq!(judge.instructions.as_deref(), Some("grade it"));
        assert_eq!(judge.criteria[0].id, "behavior");
    }

    #[test]
    fn load_rejects_missing_or_invalid_shared_judge_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(
            &path,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":"missing.judge.json"}}"#,
        )
        .unwrap();
        assert!(matches!(EvalSpec::load(&path).unwrap_err(), EvalFileError::ReadJudgeFile { .. }));

        std::fs::write(dir.path().join("bad.judge.json"), r#"{"model":"m","criteria":[]}"#).unwrap();
        std::fs::write(
            &path,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","prompt":"p","expect":{"judge":"bad.judge.json"}}"#,
        )
        .unwrap();
        let error = EvalSpec::load(&path).unwrap_err();
        assert!(matches!(&error, EvalFileError::InvalidInlineJudge { .. }));
        assert!(error.to_string().contains("criteria must not be empty"), "got: {error}");
    }

    #[test]
    fn path_settings_resolve_aether_project_resources_from_project_root() {
        let project = tempfile::tempdir().unwrap();
        let settings_dir = project.path().join(".aether");
        let eval_dir = project.path().join("packages/internal-evals/examples");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::write(settings_dir.join("SYSTEM.md"), "System prompt").unwrap();
        std::fs::write(settings_dir.join("settings.json"), r#"{"prompts":[".aether/SYSTEM.md"],"agents":[]}"#).unwrap();

        let settings = SettingsRef::Path(PathBuf::from("../../../.aether/settings.json")).resolve(&eval_dir).unwrap();

        assert_eq!(settings.prompts, vec![aether_project::PromptSource::Text { text: "System prompt".to_string() }]);
    }

    #[test]
    fn discovers_and_loads_specs_recursively() {
        let dir = tempfile::tempdir().unwrap();
        write_named_spec(dir.path().join("smoke.eval.json"), "smoke");
        write_named_spec(dir.path().join("nested/edit.eval.json"), "edit");
        write_named_spec(dir.path().join("nested/settings.json"), "settings");

        let cases = EvalSpec::load_all(&[dir.path().to_path_buf()]).unwrap();
        let names = cases.iter().map(|case| case.eval.name.clone()).collect::<Vec<_>>();

        assert_eq!(names, vec!["edit", "smoke"]);
    }

    #[test]
    fn explicit_file_path_is_loaded_regardless_of_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eval.json");
        write_named_spec(&path, "explicit");

        let cases = EvalSpec::load_all(&[path]).unwrap();

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].eval.name, "explicit");
        assert_eq!(cases[0].base_dir, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn discover_reports_missing_paths() {
        let missing = EvalSpec::load_all(&[PathBuf::from("/nonexistent/evals")]).unwrap_err();

        assert!(matches!(missing, EvalFileError::EvalPathNotFound { .. }));
    }

    fn write_named_spec(path: impl AsRef<Path>, name: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, format!(r#"{{"docker":{{"image":"sandbox:latest"}},"name":"{name}","prompt":"p"}}"#))
            .unwrap();
    }
}
