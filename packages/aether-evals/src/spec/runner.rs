use super::ResolvedEvalSpec;
use super::error::EvalFileError;
use super::judge::Judge;
use super::report::{EvalOutcome, JudgeSummary};
use crate::agents::{DockerAgent, build_images};
use crate::evals::TaskRun;
use aether_core::events::AgentMessage;
use futures::StreamExt;
use futures::stream::iter;
use llm::StreamingModelProvider;
use llm::parser::ModelProviderParser;
use std::num::NonZeroUsize;

/// Whether the eval workspace is retained on disk after the run.
#[derive(Debug, Clone, Copy)]
pub enum WorkspaceRetention {
    Discard,
    Retain,
}

/// Run resolved eval specs. Agents and judge models are assembled before anything runs, so a
/// broken case fails fast. Docker image builds are deduplicated and run concurrently before eval
/// execution. Outcomes are returned in input order.
pub async fn run_eval_specs(
    specs: impl IntoIterator<Item = ResolvedEvalSpec>,
    retention: WorkspaceRetention,
    max_concurrency: NonZeroUsize,
) -> Result<Vec<EvalOutcome>, EvalFileError> {
    let mut prepared = Vec::new();
    for spec in specs {
        prepared.push(prepare_spec(spec).await?);
    }

    build_images(prepared.iter().filter_map(|(case, _, _)| case.docker.build.clone()), max_concurrency).await?;
    let futures = prepared
        .into_iter()
        .map(|(case, agent, judge_llm)| async move { run_eval_spec(case, agent, judge_llm, retention, |_| {}).await });

    let evals = iter(futures).buffered(max_concurrency.get()).collect().await;
    Ok(evals)
}

pub async fn run_eval_spec_streaming<T: FnMut(&AgentMessage) + Send>(
    spec: ResolvedEvalSpec,
    retention: WorkspaceRetention,
    on_message: T,
) -> Result<EvalOutcome, EvalFileError> {
    let (spec, agent, judge_llm) = prepare_spec(spec).await?;
    build_images(spec.docker.build.clone(), NonZeroUsize::MIN).await?;
    Ok(run_eval_spec(spec, agent, judge_llm, retention, on_message).await)
}

async fn run_eval_spec<T: FnMut(&AgentMessage) + Send>(
    spec: ResolvedEvalSpec,
    agent: DockerAgent,
    judge_llm: Option<Box<dyn StreamingModelProvider>>,
    retention: WorkspaceRetention,
    on_message: T,
) -> EvalOutcome {
    let task = match spec.task() {
        Ok(task) => task,
        Err(error) => return EvalOutcome::setup_failed(spec.name(), error),
    };

    let run = match task.run_observed(&agent, on_message).await {
        Ok(run) => run,
        Err(error) => {
            let (run, error) = error.into_parts();
            return EvalOutcome::from_task_run(
                spec.name(),
                run,
                vec![format!("agent run failed: {error}")],
                None,
                retention,
            );
        }
    };

    let mut failures = spec.expectations().evaluate(&run);
    let judge = run_judge(&spec, &run, &mut failures, judge_llm.as_deref()).await;
    EvalOutcome::from_task_run(spec.name(), run, failures, judge, retention)
}

async fn run_judge(
    case: &ResolvedEvalSpec,
    run: &TaskRun,
    failures: &mut Vec<String>,
    judge_llm: Option<&dyn StreamingModelProvider>,
) -> Option<JudgeSummary> {
    let Some(spec) = &case.judge else {
        return None;
    };
    let llm = judge_llm.expect("judge model is parsed before any eval runs when a judge is configured");
    match Judge::new(llm, run, case.expectations(), spec).run().await {
        Ok(summary) => {
            failures.extend(summary.blocking_failures());
            Some(summary)
        }
        Err(error) => {
            failures.push(format!("judge failed: {error}"));
            None
        }
    }
}

async fn prepare_spec(
    spec: ResolvedEvalSpec,
) -> Result<(ResolvedEvalSpec, DockerAgent, Option<Box<dyn StreamingModelProvider>>), EvalFileError> {
    let agent = spec.agent();
    let judge_llm = parse_judge_llm(&spec).await?;
    Ok((spec, agent, judge_llm))
}

async fn parse_judge_llm(spec: &ResolvedEvalSpec) -> Result<Option<Box<dyn StreamingModelProvider>>, EvalFileError> {
    let Some(judge) = &spec.judge else {
        return Ok(None);
    };
    let (llm, _) = ModelProviderParser::default()
        .parse(&judge.model)
        .await
        .map_err(|source| EvalFileError::JudgeModel { model: judge.model.clone(), source })?;
    Ok(Some(llm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeAgent;
    use crate::agents::{DockerImage, ImageBuildError};
    use crate::spec::{EvalFileError, EvalSpec, EvalSpecLoadOptions, ResolvedEvalSpec};
    use std::fs::{read_to_string, remove_dir_all};
    use std::path::{Path, PathBuf};

    fn eval_case(json: &str) -> ResolvedEvalSpec {
        ResolvedEvalSpec::resolve(serde_json::from_value(eval_json_with_defaults(json)).unwrap(), ".").unwrap()
    }

    fn eval_case_in(json: &str, base_dir: &Path) -> ResolvedEvalSpec {
        ResolvedEvalSpec::resolve(serde_json::from_value(eval_json_with_defaults(json)).unwrap(), base_dir).unwrap()
    }

    fn eval_json_with_defaults(json: &str) -> serde_json::Value {
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        if let serde_json::Value::Object(object) = &mut value {
            object.entry("docker").or_insert_with(|| serde_json::json!({ "image": "sandbox:latest" }));
            object.entry("agent").or_insert_with(|| serde_json::json!({ "command": ["agent"] }));
        }
        value
    }

    async fn run_case_with_agent(
        case: ResolvedEvalSpec,
        agent: &impl crate::Agent,
        retention: WorkspaceRetention,
    ) -> EvalOutcome {
        let task = match case.task() {
            Ok(task) => task,
            Err(error) => return EvalOutcome::setup_failed(case.name(), error),
        };
        match task.run(agent).await {
            Ok(run) => {
                let failures = case.expectations().evaluate(&run);
                EvalOutcome::from_task_run(case.name(), run, failures, None, retention)
            }
            Err(error) => {
                let (run, error) = error.into_parts();
                EvalOutcome::from_task_run(
                    case.name(),
                    run,
                    vec![format!("agent run failed: {error}")],
                    None,
                    retention,
                )
            }
        }
    }

    #[tokio::test]
    async fn run_passes_when_all_expectations_met() {
        let case = eval_case(
            r#"{"name":"expectations","task":{"prompt":"do the thing","workspace":{"files":{"notes.txt":"beta\n"}}},"expect":{"toolCalls":{"bash":{"exactly":1}},"files":{"notes.txt":"beta\n"},"filesContain":{"notes.txt":"beta"}}}"#,
        );

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Discard).await;

        assert!(outcome.passed, "failures: {:?}", outcome.failures);
    }

    #[tokio::test]
    async fn run_reports_tool_calls_for_native_assertions() {
        let case = eval_case(r#"{"name":"observations","task":{"prompt":"p"}}"#);

        let outcome = run_case_with_agent(
            case,
            &FakeAgent::with_tool_call("weather__get_current", "ok"),
            WorkspaceRetention::Discard,
        )
        .await;

        assert_eq!(outcome.tool_calls.len(), 1);
        assert_eq!(outcome.tool_calls[0].name, "weather__get_current");
        assert_eq!(outcome.tool_calls[0].arguments, Some(serde_json::json!({})));
        assert_eq!(outcome.tool_calls[0].raw_arguments, "{}");
    }

    #[tokio::test]
    async fn run_keep_workspace_persists_directory_and_reports_path() {
        let case = eval_case(r#"{"name":"keep","task":{"prompt":"p","workspace":{"files":{"notes.txt":"hi\n"}}}}"#);

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Retain).await;

        let workspace = outcome.retained_workspace.expect("workspace is reported when retained");
        assert!(workspace.root_path.exists(), "retained workspace should outlive the run");
        assert_eq!(workspace.root_path, workspace.path);
        assert_eq!(read_to_string(workspace.path.join("notes.txt")).unwrap(), "hi\n");
        remove_dir_all(&workspace.root_path).expect("caller cleans up the retained workspace");
    }

    #[tokio::test]
    async fn run_reports_no_retained_workspace_when_not_retained() {
        let case = eval_case(r#"{"name":"drop","task":{"prompt":"p","workspace":{"files":{"a.txt":"x"}}}}"#);

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Discard).await;

        assert!(outcome.retained_workspace.is_none());
    }

    #[tokio::test]
    async fn run_reports_missing_tool_and_wrong_count() {
        let case = eval_case(
            r#"{"name":"counts","task":{"prompt":"p"},"expect":{"toolCalls":{"read":{"atLeast":1},"bash":{"exactly":2}}}}"#,
        );

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Discard).await;

        assert!(!outcome.passed);
        assert!(outcome.failures.iter().any(|f| f.contains("`read`")));
        assert!(outcome.failures.iter().any(|f| f.contains("`bash`") && f.contains("2 time")));
    }

    #[tokio::test]
    async fn run_reports_file_mismatch_and_missing_file() {
        let case = eval_case(
            r#"{"name":"files","task":{"prompt":"p","workspace":{"files":{"notes.txt":"alpha\n"}}},"expect":{"files":{"notes.txt":"beta\n"},"filesContain":{"missing.txt":"x"}}}"#,
        );

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Discard).await;

        assert!(!outcome.passed);
        assert!(outcome.failures.iter().any(|f| f.contains("`notes.txt`") && f.contains("mismatch")));
        assert!(outcome.failures.iter().any(|f| f.contains("`missing.txt`") && f.contains("could not be read")));
    }

    #[tokio::test]
    async fn run_records_fault_as_failed_eval_instead_of_aborting() {
        let base_dir = tempfile::tempdir().unwrap();
        let case = eval_case_in(
            r#"{"name":"broken","task":{"prompt":"p","workspace":{"dir":"missing-dir"}}}"#,
            base_dir.path(),
        );

        let outcome =
            run_case_with_agent(case, &FakeAgent::with_tool_call("bash", "ok"), WorkspaceRetention::Discard).await;

        assert!(!outcome.passed);
        assert!(outcome.failures[0].contains("workspace setup failed"), "got: {:?}", outcome.failures);
    }

    #[test]
    fn load_rejects_missing_docker_section() {
        let error = serde_json::from_str::<EvalSpec>(r#"{"name":"c","task":{"prompt":"p"}}"#).unwrap_err();

        assert!(error.to_string().contains("missing field `docker`"), "got: {error}");
    }

    #[test]
    fn load_rejects_missing_dockerfile() {
        let base_dir = tempfile::tempdir().unwrap();
        let eval: EvalSpec = serde_json::from_str(
            r#"{"docker":{"file":"Dockerfile"},"name":"x","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#,
        )
        .unwrap();

        let error = ResolvedEvalSpec::resolve(eval, base_dir.path()).unwrap_err();

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
                format!(
                    r#"{{"docker":{{"file":"Dockerfile","image":"same:tag"}},"name":"{name}","agent":{{"command":["agent"]}},"task":{{"prompt":"p"}}}}"#
                ),
            )
            .unwrap();
        }
        let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: vec![dir.path().to_path_buf()], filter: None })
            .unwrap();
        let error =
            run_eval_specs(cases, WorkspaceRetention::Discard, NonZeroUsize::new(2).unwrap()).await.unwrap_err();

        assert!(matches!(error, EvalFileError::ImageBuild(ImageBuildError::ConflictingTag { .. })));
    }

    #[tokio::test]
    async fn run_evals_rejects_invalid_judge_model_before_running() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad-judge.eval.json"),
            r#"{"docker":{"image":"sandbox:latest"},"name":"bad-judge","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":{"model":"not-a-provider:nope","criteria":[{"id":"a","description":"ok"}]}}}"#,
        )
        .unwrap();

        let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: vec![dir.path().to_path_buf()], filter: None })
            .unwrap();
        let error =
            run_eval_specs(cases, WorkspaceRetention::Discard, NonZeroUsize::new(1).unwrap()).await.unwrap_err();

        assert!(matches!(error, EvalFileError::JudgeModel { .. }), "got: {error:?}");
    }

    #[test]
    fn load_reads_spec_and_returns_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(
            &path,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#,
        )
        .unwrap();

        let case = ResolvedEvalSpec::load_file(&path).unwrap();

        let docker = &case.docker;
        assert_eq!(docker.image, DockerImage::new("sandbox", "latest"));
        assert!(docker.build.is_none());
        assert_eq!(case.name, "c");
        assert_eq!(case.base_dir, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn load_resolves_dockerfile_build_relative_to_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch").unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(
            &path,
            r#"{"docker":{"file":"Dockerfile","image":"sandbox:dev"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"}}"#,
        )
        .unwrap();

        let case = ResolvedEvalSpec::load_file(&path).unwrap();

        let docker = &case.docker;
        assert_eq!(docker.image, DockerImage::new("sandbox", "dev"));
        let build = docker.build.as_ref().expect("expected a build request");
        assert_eq!(build.dockerfile, dir.path().canonicalize().unwrap().join("Dockerfile"));
        assert_eq!(build.context, dir.path().canonicalize().unwrap());
        assert_eq!(build.tag, "sandbox:dev");
    }

    #[test]
    fn load_distinguishes_missing_file_from_invalid_json() {
        let missing = ResolvedEvalSpec::load_file("/nonexistent/example.eval.json").unwrap_err();
        assert!(matches!(missing, EvalFileError::ReadEvalFile { .. }));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.eval.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(ResolvedEvalSpec::load_file(&path).unwrap_err(), EvalFileError::ParseEvalFile { .. }));
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
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":"../shared/maintainer.judge.json"}}"#,
        )
        .unwrap();

        let case = ResolvedEvalSpec::load_file(&path).unwrap();

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
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":"missing.judge.json"}}"#,
        )
        .unwrap();
        assert!(matches!(ResolvedEvalSpec::load_file(&path).unwrap_err(), EvalFileError::ReadJudgeFile { .. }));

        std::fs::write(dir.path().join("bad.judge.json"), r#"{"model":"m","criteria":[]}"#).unwrap();
        std::fs::write(
            &path,
            r#"{"docker":{"image":"sandbox:latest"},"name":"c","agent":{"command":["agent"]},"task":{"prompt":"p"},"expect":{"judge":"bad.judge.json"}}"#,
        )
        .unwrap();
        let error = ResolvedEvalSpec::load_file(&path).unwrap_err();
        assert!(matches!(&error, EvalFileError::InvalidInlineJudge { .. }));
        assert!(error.to_string().contains("criteria must not be empty"), "got: {error}");
    }

    #[test]
    fn discovers_and_loads_specs_recursively() {
        let dir = tempfile::tempdir().unwrap();
        write_named_spec(dir.path().join("smoke.eval.json"), "smoke");
        write_named_spec(dir.path().join("nested/edit.eval.json"), "edit");
        write_named_spec(dir.path().join("nested/settings.json"), "settings");

        let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: vec![dir.path().to_path_buf()], filter: None })
            .unwrap();
        let names = cases.iter().map(|case| case.name.clone()).collect::<Vec<_>>();

        assert_eq!(names, vec!["edit", "smoke"]);
    }

    #[test]
    fn explicit_file_path_is_loaded_regardless_of_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eval.json");
        write_named_spec(&path, "explicit");

        let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: vec![path], filter: None }).unwrap();

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "explicit");
        assert_eq!(cases[0].base_dir, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn load_filters_by_eval_name() {
        let dir = tempfile::tempdir().unwrap();
        write_named_spec(dir.path().join("smoke.eval.json"), "smoke");
        write_named_spec(dir.path().join("edit.eval.json"), "edit");

        let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions {
            paths: vec![dir.path().to_path_buf()],
            filter: Some("smoke".to_string()),
        })
        .unwrap();

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "smoke");
    }

    #[test]
    fn load_reports_missing_filter_match() {
        let dir = tempfile::tempdir().unwrap();
        write_named_spec(dir.path().join("smoke.eval.json"), "smoke");

        let error = ResolvedEvalSpec::load(EvalSpecLoadOptions {
            paths: vec![dir.path().to_path_buf()],
            filter: Some("missing".to_string()),
        })
        .unwrap_err();

        assert!(matches!(error, EvalFileError::NoMatchingEval { .. }));
    }

    #[test]
    fn discover_reports_missing_paths() {
        let missing = ResolvedEvalSpec::load(EvalSpecLoadOptions {
            paths: vec![PathBuf::from("/nonexistent/evals")],
            filter: None,
        })
        .unwrap_err();

        assert!(matches!(missing, EvalFileError::EvalPathNotFound { .. }));
    }

    fn write_named_spec(path: impl AsRef<Path>, name: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            path,
            format!(r#"{{"docker":{{"image":"sandbox:latest"}},"name":"{name}","agent":{{"command":["agent"]}},"task":{{"prompt":"p"}}}}"#),
        )
        .unwrap();
    }
}
