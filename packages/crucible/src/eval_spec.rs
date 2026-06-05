use crate::error::EvalSpecError;
use crate::evals::{GitRepoSpec, LlmJudgeContext, Workspace, run_eval};
use crate::metrics::EvalMetric;
use crate::{Agent, AgentEvalMessage, EvalReport};
use llm::StreamingModelProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Run an [`EvalSpec`] end to end: build the workspace, run the agent, then
/// evaluate every `expect` entry against the resulting [`EvalReport`].
///
/// `base_dir` resolves relative paths in the spec (`workspace.dir`,
/// `prompt.file`); it is typically the directory containing the spec file.
/// `judge_llm` backs any `judge` expectations.
pub async fn run_spec(
    spec: &EvalSpec,
    base_dir: &Path,
    agent: &impl Agent,
    judge_llm: &dyn StreamingModelProvider,
) -> Result<SpecReport, EvalSpecError> {
    let workspace = spec.workspace.build(base_dir)?;
    let prompt = spec.prompt.resolve(base_dir)?;
    let report = run_eval(agent, prompt, workspace).await?;

    let mut results = Vec::new();
    for expectation in &spec.expect {
        results.push(evaluate(expectation, &report, judge_llm).await?);
    }
    Ok(SpecReport { results })
}

/// Result of evaluating an entire [`EvalSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecReport {
    pub results: Vec<CheckResult>,
}

impl SpecReport {
    pub fn passed(&self) -> bool {
        self.results.iter().all(|result| result.passed)
    }
}

/// Outcome of a single `expect` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

impl CheckResult {
    fn new(label: String, passed: bool, detail: impl Into<String>) -> Self {
        Self { label, passed, detail: detail.into() }
    }
}

/// A declarative eval definition — the user-facing JSON format.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalSpec {
    /// Named agent from `settings.json`.
    #[serde(default)]
    pub agent: Option<String>,
    /// Ad-hoc `provider:model` spec (mutually exclusive with `agent`).
    #[serde(default)]
    pub model: Option<String>,
    /// Optional isolated container the eval runs inside. Orchestrated by the
    /// CLI; ignored by [`run_spec`], which evaluates against the workspace path
    /// regardless of where it is mounted.
    #[serde(default)]
    pub environment: Option<EnvironmentSpec>,
    /// The workspace the agent operates in. Omit for an empty workspace.
    #[serde(default)]
    pub workspace: WorkspaceSpec,
    /// The task prompt sent to the agent.
    pub prompt: PromptSpec,
    /// Assertions evaluated after the agent finishes.
    pub expect: Vec<Expectation>,
    /// Override the model used for `judge` expectations (defaults to the agent's).
    #[serde(default)]
    pub judge_model: Option<String>,
}

/// Isolated container environment for an eval.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum EnvironmentSpec {
    Dockerfile { dockerfile: PathBuf },
    Image { image: String },
}

/// Where the agent's workspace comes from.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum WorkspaceSpec {
    /// Write these `path -> contents` fixtures into an empty workspace.
    Files { files: BTreeMap<String, String> },
    /// Copy a local fixture directory (relative to the spec file).
    Dir { dir: PathBuf },
    /// Clone a git repo at `start`, with `gold` as the human-solution reference.
    Git { git: GitSpec },
    /// An empty workspace. Must be last so it only matches `{}`.
    Empty {},
}

impl Default for WorkspaceSpec {
    fn default() -> Self {
        WorkspaceSpec::Empty {}
    }
}

/// A git-repo workspace. `start` is the base commit the agent begins from;
/// `gold` is the human-completed solution commit (drives the reference diff).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSpec {
    pub url: String,
    pub start: String,
    pub gold: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
}

/// The task prompt: a string, an array of lines (joined with newlines), or a file.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PromptSpec {
    Text(String),
    Lines(Vec<String>),
    File { file: PathBuf },
}

/// A single assertion. Discriminated by which key is present.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum Expectation {
    /// Assert on the agent's tool calls.
    Tool {
        tool: String,
        #[serde(default)]
        count: Option<usize>,
        #[serde(default)]
        args: Option<Value>,
    },
    /// Assert on a file in the workspace.
    File {
        file: String,
        #[serde(default)]
        equals: Option<String>,
        #[serde(default)]
        contains: Option<String>,
        #[serde(default)]
        exists: Option<bool>,
    },
    /// Run a shell command in the workspace and assert on its result.
    Run {
        run: String,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        stdout_contains: Option<String>,
    },
    /// Ask an LLM judge to evaluate a natural-language criterion.
    Judge {
        judge: String,
        #[serde(default)]
        metric: Option<MetricSpec>,
    },
}

/// Scoring mode for a `judge` expectation. Absent means a binary pass/fail.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetricSpec {
    Numeric(NumericThreshold),
}

/// Minimum `score / max_score` ratio for a numeric judge to pass.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NumericThreshold {
    pub min: f64,
}

impl EvalSpec {
    pub fn parse(json: &str) -> Result<Self, EvalSpecError> {
        serde_json::from_str(json).map_err(EvalSpecError::Json)
    }
}

impl WorkspaceSpec {
    fn build(&self, base_dir: &Path) -> Result<Workspace, EvalSpecError> {
        match self {
            WorkspaceSpec::Empty {} => Ok(Workspace::empty()?),
            WorkspaceSpec::Files { files } => {
                let workspace = Workspace::empty()?;
                for (relative, contents) in files {
                    let path = workspace.path().join(relative);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|source| EvalSpecError::Fixture { path: parent.to_path_buf(), source })?;
                    }
                    std::fs::write(&path, contents).map_err(|source| EvalSpecError::Fixture { path, source })?;
                }
                Ok(workspace)
            }
            WorkspaceSpec::Dir { dir } => Ok(Workspace::from_dir(base_dir.join(dir))?),
            WorkspaceSpec::Git { git } => Ok(Workspace::from_git_repo(GitRepoSpec {
                url: git.url.clone(),
                start_commit: git.start.clone(),
                gold_commit: git.gold.clone(),
                subdir: git.subdir.clone(),
            })?),
        }
    }
}

impl PromptSpec {
    fn resolve(&self, base_dir: &Path) -> Result<String, EvalSpecError> {
        match self {
            PromptSpec::Text(text) => Ok(text.clone()),
            PromptSpec::Lines(lines) => Ok(lines.join("\n")),
            PromptSpec::File { file } => {
                let path = base_dir.join(file);
                std::fs::read_to_string(&path).map_err(|source| EvalSpecError::PromptFile { path, source })
            }
        }
    }
}

async fn evaluate(
    expectation: &Expectation,
    report: &EvalReport,
    judge_llm: &dyn StreamingModelProvider,
) -> Result<CheckResult, EvalSpecError> {
    match expectation {
        Expectation::Tool { tool, count, args } => Ok(evaluate_tool(report, tool, *count, args.as_ref())),
        Expectation::File { file, equals, contains, exists } => {
            Ok(evaluate_file(report, file, equals.as_deref(), contains.as_deref(), *exists))
        }
        Expectation::Run { run, exit_code, stdout_contains } => {
            evaluate_run(report, run, *exit_code, stdout_contains.as_deref()).await
        }
        Expectation::Judge { judge, metric } => evaluate_judge(report, judge, metric.as_ref(), judge_llm).await,
    }
}

fn evaluate_tool(report: &EvalReport, tool: &str, count: Option<usize>, args: Option<&Value>) -> CheckResult {
    let label = format!("tool {tool}");

    if let Some(expected) = count {
        let actual = report.tool_call_count(tool);
        return CheckResult::new(label, actual == expected, format!("expected {expected} call(s), saw {actual}"));
    }

    if let Some(expected_args) = args {
        let matched =
            report.tool_calls(tool).any(|call| call.arguments_json().is_ok_and(|actual| actual == *expected_args));
        let detail = if matched {
            format!("called with args {expected_args}")
        } else {
            format!("no call to {tool} with args {expected_args}")
        };
        return CheckResult::new(label, matched, detail);
    }

    let called = report.tool_called(tool);
    let detail = if called { "called".to_string() } else { format!("{tool} was not called") };
    CheckResult::new(label, called, detail)
}

fn evaluate_file(
    report: &EvalReport,
    file: &str,
    equals: Option<&str>,
    contains: Option<&str>,
    exists: Option<bool>,
) -> CheckResult {
    let label = format!("file {file}");
    let path = report.path(file);

    if let Some(expected_exists) = exists {
        let actual = path.exists();
        return CheckResult::new(
            label,
            actual == expected_exists,
            format!("exists={actual}, expected {expected_exists}"),
        );
    }

    if let Some(expected) = equals {
        return match std::fs::read_to_string(&path) {
            Ok(contents) if contents == expected => CheckResult::new(label, true, "contents match"),
            Ok(contents) => CheckResult::new(label, false, format!("contents differ ({} bytes)", contents.len())),
            Err(error) => CheckResult::new(label, false, format!("could not read {file}: {error}")),
        };
    }

    if let Some(needle) = contains {
        return match std::fs::read_to_string(&path) {
            Ok(contents) if contents.contains(needle) => CheckResult::new(label, true, format!("contains {needle:?}")),
            Ok(_) => CheckResult::new(label, false, format!("does not contain {needle:?}")),
            Err(error) => CheckResult::new(label, false, format!("could not read {file}: {error}")),
        };
    }

    let exists_now = path.exists();
    CheckResult::new(label, exists_now, format!("exists={exists_now}"))
}

async fn evaluate_run(
    report: &EvalReport,
    cmd: &str,
    exit_code: Option<i32>,
    stdout_contains: Option<&str>,
) -> Result<CheckResult, EvalSpecError> {
    let label = format!("run `{cmd}`");
    let outcome = run_in_workspace(report.workspace().path(), cmd)
        .await
        .map_err(|source| EvalSpecError::Command { cmd: cmd.to_string(), source })?;

    let mut failures = Vec::new();
    let expected_code = exit_code.unwrap_or(0);
    if outcome.exit_code != expected_code {
        failures.push(format!("exit {} != {expected_code}", outcome.exit_code));
    }
    if let Some(needle) = stdout_contains
        && !outcome.stdout.contains(needle)
    {
        failures.push(format!("stdout missing {needle:?}"));
    }

    let passed = failures.is_empty();
    let detail = if passed {
        format!("exit {}", outcome.exit_code)
    } else {
        let mut detail = failures.join("; ");
        if !outcome.stderr.trim().is_empty() {
            let _ = write!(detail, " | stderr: {}", truncate(&outcome.stderr, 500));
        }
        detail
    };
    Ok(CheckResult::new(label, passed, detail))
}

async fn evaluate_judge(
    report: &EvalReport,
    criterion: &str,
    metric: Option<&MetricSpec>,
    judge_llm: &dyn StreamingModelProvider,
) -> Result<CheckResult, EvalSpecError> {
    let label = format!("judge: {}", truncate(criterion, 60));
    let numeric_min = metric.map(|MetricSpec::Numeric(threshold)| threshold.min);

    let judgment = report.judge(judge_llm, |ctx| build_judge_prompt(criterion, numeric_min, ctx)).await?;

    let passed = match numeric_min {
        Some(min) => match serde_json::from_str::<EvalMetric>(judgment.raw_response().trim()) {
            Ok(EvalMetric::Numeric(numeric)) if numeric.max_score != 0.0 => numeric.score / numeric.max_score >= min,
            _ => judgment.passed(),
        },
        None => judgment.passed(),
    };

    Ok(CheckResult::new(label, passed, judgment.reason().to_string()))
}

fn build_judge_prompt(criterion: &str, numeric_min: Option<f64>, ctx: &LlmJudgeContext) -> String {
    let mut prompt = String::from("You are evaluating an AI coding agent's work against a criterion.\n\n");
    let _ = write!(prompt, "Task given to the agent:\n{}\n\n", ctx.original_prompt);
    let _ = write!(prompt, "Agent transcript (what the agent actually did):\n{}\n\n", render_transcript(ctx.messages));
    if let Some(diff) = ctx.git_diff(None) {
        let _ = write!(prompt, "The agent produced this diff:\n```diff\n{diff}\n```\n\n");
    }
    let _ = write!(prompt, "Criterion to evaluate:\n{criterion}\n\n");
    if numeric_min.is_some() {
        prompt.push_str("Respond with ONLY a JSON object of this exact shape, nothing else:\n");
        prompt.push_str(
            r#"{"type":"numeric","score":<number 0..max_score>,"max_score":<number>,"reason":"<short explanation>"}"#,
        );
    } else {
        prompt.push_str("Respond with ONLY a JSON object of this exact shape, nothing else:\n");
        prompt.push_str(r#"{"type":"binary","success":<true|false>,"reason":"<short explanation>"}"#);
    }
    prompt
}

fn render_transcript(messages: &[AgentEvalMessage]) -> String {
    if messages.is_empty() {
        return "(no messages)".to_string();
    }
    let mut transcript = String::new();
    for message in messages {
        match message {
            AgentEvalMessage::AgentText(text) => {
                let _ = writeln!(transcript, "[text] {}", truncate(text, 1_000));
            }
            AgentEvalMessage::ToolCall { name, arguments } => {
                let _ = writeln!(transcript, "[tool-call] {name} {}", truncate(arguments, 500));
            }
            AgentEvalMessage::ToolResult { name, result } => {
                let _ = writeln!(transcript, "[tool-result] {name}: {}", truncate(result, 500));
            }
            AgentEvalMessage::ToolError(error) => {
                let _ = writeln!(transcript, "[tool-error] {}", truncate(error, 500));
            }
            AgentEvalMessage::Error(error) => {
                let _ = writeln!(transcript, "[error] {}", truncate(error, 500));
            }
            AgentEvalMessage::Done => {}
        }
    }
    transcript
}

struct CommandOutcome {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

async fn run_in_workspace(cwd: &Path, cmd: &str) -> Result<CommandOutcome, std::io::Error> {
    let output = tokio::process::Command::new("sh").arg("-c").arg(cmd).current_dir(cwd).output().await?;
    Ok(CommandOutcome {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeAgent;
    use llm::LlmResponse;
    use llm::testing::FakeLlmProvider;

    fn unused_judge() -> FakeLlmProvider {
        FakeLlmProvider::with_single_response(vec![LlmResponse::text("{}")])
    }

    #[test]
    fn parses_trajectory_example() {
        let json = r#"{
          "agent": "Build",
          "workspace": { "files": { "notes.txt": "alpha\nalpha\n" } },
          "prompt": ["Update notes.txt:", "replace only the first 'alpha' with 'beta'."],
          "expect": [
            { "tool": "coding__edit_file", "count": 1 },
            { "file": "notes.txt", "equals": "beta\nalpha\n" }
          ]
        }"#;

        let spec = EvalSpec::parse(json).unwrap();

        assert_eq!(spec.agent.as_deref(), Some("Build"));
        assert!(matches!(spec.workspace, WorkspaceSpec::Files { .. }));
        assert!(matches!(spec.prompt, PromptSpec::Lines(_)));
        assert_eq!(spec.expect.len(), 2);
        assert!(matches!(spec.expect[0], Expectation::Tool { count: Some(1), .. }));
        assert!(matches!(spec.expect[1], Expectation::File { .. }));
    }

    #[test]
    fn parses_outcome_example_with_environment_and_git() {
        let json = r#"{
          "agent": "Build",
          "environment": { "dockerfile": "environment/Dockerfile" },
          "workspace": { "git": { "url": "https://example.com/r.git", "start": "aaa", "gold": "bbb" } },
          "prompt": { "file": "issues/1.md" },
          "expect": [
            { "run": "just test", "exitCode": 0 },
            { "judge": "The fix is clean." },
            { "judge": "Rate style.", "metric": { "numeric": { "min": 0.7 } } }
          ]
        }"#;

        let spec = EvalSpec::parse(json).unwrap();

        assert!(matches!(spec.environment, Some(EnvironmentSpec::Dockerfile { .. })));
        assert!(matches!(spec.workspace, WorkspaceSpec::Git { .. }));
        assert!(matches!(spec.prompt, PromptSpec::File { .. }));
        assert!(matches!(spec.expect[0], Expectation::Run { exit_code: Some(0), .. }));
        assert!(matches!(spec.expect[1], Expectation::Judge { metric: None, .. }));
        assert!(matches!(spec.expect[2], Expectation::Judge { metric: Some(MetricSpec::Numeric(_)), .. }));
    }

    #[test]
    fn workspace_defaults_to_empty_when_omitted() {
        let spec = EvalSpec::parse(r#"{ "prompt": "do it", "expect": [] }"#).unwrap();
        assert!(matches!(spec.workspace, WorkspaceSpec::Empty {}));
    }

    #[tokio::test]
    async fn run_spec_evaluates_tool_and_file_checks() {
        let json = r#"{
          "workspace": { "files": { "seed.txt": "x" } },
          "prompt": "go",
          "expect": [
            { "tool": "bash", "count": 1 },
            { "tool": "missing", "count": 0 },
            { "file": "out.txt", "equals": "hello" },
            { "file": "nope.txt", "exists": false }
          ]
        }"#;
        let spec = EvalSpec::parse(json).unwrap();
        let agent = FakeAgent::with_tool_call("bash", "ok").with_file_write("out.txt", "hello");

        let report = run_spec(&spec, Path::new("."), &agent, &unused_judge()).await.unwrap();

        assert!(report.passed(), "{:?}", report.results);
    }

    #[tokio::test]
    async fn run_spec_run_command_checks_exit_code() {
        let json = r#"{
          "prompt": "go",
          "expect": [
            { "run": "test -f made.txt" },
            { "run": "test -f absent.txt" }
          ]
        }"#;
        let spec = EvalSpec::parse(json).unwrap();
        let agent = FakeAgent::writes_file("made.txt", "hi");

        let report = run_spec(&spec, Path::new("."), &agent, &unused_judge()).await.unwrap();

        assert!(report.results[0].passed, "present file: {:?}", report.results[0]);
        assert!(!report.results[1].passed, "absent file: {:?}", report.results[1]);
    }

    #[tokio::test]
    async fn run_spec_binary_judge_uses_response() {
        let spec = EvalSpec::parse(r#"{ "prompt": "go", "expect": [ { "judge": "ok?" } ] }"#).unwrap();
        let agent = FakeAgent::success();
        let judge = FakeLlmProvider::with_single_response(vec![LlmResponse::text(
            r#"{"type":"binary","success":true,"reason":"great"}"#,
        )]);

        let report = run_spec(&spec, Path::new("."), &agent, &judge).await.unwrap();

        assert!(report.passed());
        assert_eq!(report.results[0].detail, "great");
    }

    #[tokio::test]
    async fn run_spec_numeric_judge_applies_custom_min() {
        let spec = EvalSpec::parse(
            r#"{ "prompt": "go", "expect": [ { "judge": "score it", "metric": { "numeric": { "min": 0.9 } } } ] }"#,
        )
        .unwrap();
        let agent = FakeAgent::success();
        // 0.8 ratio clears the built-in 0.7 default but must fail our explicit 0.9 min.
        let judge = FakeLlmProvider::with_single_response(vec![LlmResponse::text(
            r#"{"type":"numeric","score":8.0,"max_score":10.0,"reason":"decent"}"#,
        )]);

        let report = run_spec(&spec, Path::new("."), &agent, &judge).await.unwrap();

        assert!(!report.passed(), "0.8 should fail min 0.9: {:?}", report.results);
    }
}
