use super::error::EvalFileError;
use crate::agents::{DockerImage, ImageBuildRequest};
use crate::{GitRepoSpec, Task, TaskRun, Workspace, WorkspaceError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Docker image configuration for an eval sandbox: either a prebuilt `image`, or a Dockerfile
/// `file` to build (optionally tagged with `image`).
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "DockerSpecRepr")]
pub enum DockerSpec {
    /// Prebuilt sandbox image to run the eval in. The image must have `aether` on its `PATH`.
    Prebuilt { image: String },
    /// Dockerfile (relative to the eval file) to build into an image. When `image` is set, the
    /// built image is tagged with it; otherwise the tag is derived from the Dockerfile and
    /// context paths. `context` is relative to the eval file and defaults to its directory.
    Build { file: PathBuf, image: Option<String>, context: Option<PathBuf> },
}

/// A [`DockerSpec`] resolved against the eval file's directory: the image to run in and, for
/// Dockerfile-backed specs, the build that produces it.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedDocker {
    pub image: DockerImage,
    pub build: Option<ImageBuildRequest>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSpec {
    pub prompt: String,
    #[serde(default)]
    pub workspace: WorkspaceSpec,
}

/// The starting workspace for an eval. Omitted means an empty temporary directory.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceSpec {
    #[default]
    Empty,
    /// Inline files written into a fresh workspace, keyed by relative path.
    Files(BTreeMap<String, String>),
    /// A directory (relative to the eval file) copied into a fresh workspace.
    Dir(PathBuf),
    /// A git repository checked out at `startCommit`.
    Git(GitRepoSpec),
}

/// An LLM-as-judge rubric, either inline or a path (relative to the eval file) to a shared JSON
/// file holding a [`JudgeSpec`], so one rubric can be reused across evals.
#[derive(Debug, Clone)]
pub enum PathOrInline<T> {
    Path(PathBuf),
    Inline(T),
}

pub type JudgeRef = PathOrInline<JudgeSpec>;

/// An LLM-as-judge rubric for an eval.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgeSpec {
    /// Model to use for this judge check (e.g. "anthropic:claude-sonnet-4-5").
    pub model: String,

    /// Optional high-level grading instructions for the LLM judge.
    #[serde(default)]
    pub instructions: Option<String>,

    /// Workspace-relative files to include in the judge context in addition to deterministic
    /// expectation files.
    #[serde(default)]
    pub context_files: Vec<String>,

    /// Ordered rubric criteria to score on a normalized 0.0..=1.0 scale.
    #[serde(default)]
    pub criteria: Vec<JudgeCriterionSpec>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolCallExpectation {
    AtLeast(usize),
    Exactly(usize),
}

/// Expectations checked against the agent's run. All are optional; an empty `expect` passes as
/// long as the agent runs to completion.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expect {
    /// Tool call count requirements by tool name.
    #[serde(default)]
    pub tool_calls: BTreeMap<String, ToolCallExpectation>,

    /// Files whose full content must equal the given value, keyed by workspace-relative path.
    #[serde(default)]
    pub files: BTreeMap<String, String>,

    /// Files that must contain the given substring, keyed by workspace-relative path.
    #[serde(default)]
    pub files_contain: BTreeMap<String, String>,

    /// LLM-as-judge evaluation, inline or a path to a shared judge file.
    #[serde(default)]
    pub judge: Option<JudgeRef>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSpec {
    pub command: Vec<String>,

    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl AgentSpec {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let Some(program) = self.command.first() else {
            return Err("agent command must not be empty".to_string());
        };
        if program.trim().is_empty() {
            return Err("agent command program must not be empty".to_string());
        }
        for key in self.env.keys() {
            if key.trim().is_empty() || key.contains('=') {
                return Err(format!("agent env contains invalid key `{key}`"));
            }
        }
        Ok(())
    }
}

impl DockerSpec {
    /// Resolve the image to run in, and the build producing it for Dockerfile-backed specs, with
    /// paths resolved relative to `base_dir`.
    pub(crate) fn resolve(&self, base_dir: &Path) -> Result<ResolvedDocker, EvalFileError> {
        match self {
            Self::Prebuilt { image } => Ok(ResolvedDocker { image: DockerImage::parse(image)?, build: None }),
            Self::Build { file, image, context } => {
                let dockerfile = canonical_docker_path(base_dir.join(file))?;
                let context = canonical_docker_path(
                    context.as_ref().map_or_else(|| base_dir.to_path_buf(), |context| base_dir.join(context)),
                )?;
                let tag = image.clone().unwrap_or_else(|| derived_tag(&dockerfile, &context));
                Ok(ResolvedDocker {
                    image: DockerImage::parse(&tag)?,
                    build: Some(ImageBuildRequest { dockerfile, context, tag }),
                })
            }
        }
    }
}

impl TaskSpec {
    pub(crate) fn build(&self, base_dir: &Path) -> Result<Task, WorkspaceError> {
        Ok(Task::new(self.prompt.clone(), self.workspace.build(base_dir)?))
    }
}

impl WorkspaceSpec {
    pub(crate) fn build(&self, base_dir: &Path) -> Result<Workspace, WorkspaceError> {
        match self {
            Self::Empty => Workspace::empty(),
            Self::Files(files) => Workspace::from_files(files),
            Self::Dir(dir) => Workspace::from_dir(base_dir.join(dir)),
            Self::Git(git) => Workspace::from_git_repo(git.clone()),
        }
    }
}

impl Expect {
    pub(crate) fn evaluate(&self, run: &TaskRun) -> Vec<String> {
        let mut failures = Vec::new();

        for (tool, expectation) in &self.tool_calls {
            let actual = run.transcript().tool_call_count(tool);
            if let Some(failure) = expectation.failure(tool, actual) {
                failures.push(failure);
            }
        }

        for (path, expected) in &self.files {
            match read_workspace_file(run, path) {
                Ok(actual) if &actual == expected => {}
                Ok(actual) => failures
                    .push(format!("file `{path}` content mismatch:\n  expected: {expected:?}\n  actual:   {actual:?}")),
                Err(error) => failures.push(format!("file `{path}` could not be read: {error}")),
            }
        }

        for (path, needle) in &self.files_contain {
            match read_workspace_file(run, path) {
                Ok(actual) if actual.contains(needle) => {}
                Ok(_) => failures.push(format!("file `{path}` does not contain {needle:?}")),
                Err(error) => failures.push(format!("file `{path}` could not be read: {error}")),
            }
        }

        failures
    }
}

fn read_workspace_file(run: &TaskRun, relative_path: &str) -> std::io::Result<String> {
    std::fs::read_to_string(run.workspace().join(relative_path))
}

impl JudgeRef {
    /// Resolve to a concrete judge spec, reading path references relative to `base_dir`.
    pub fn resolve(&self, base_dir: &Path) -> Result<JudgeSpec, EvalFileError> {
        match self {
            JudgeRef::Inline(spec) => {
                spec.validate().map_err(|message| EvalFileError::InvalidInlineJudge { message })?;
                Ok(spec.clone())
            }
            JudgeRef::Path(path) => {
                let path = base_dir.join(path);
                let content = std::fs::read_to_string(&path)
                    .map_err(|source| EvalFileError::ReadJudgeFile { path: path.clone(), source })?;
                let spec: JudgeSpec = serde_json::from_str(&content)
                    .map_err(|source| EvalFileError::ParseJudgeFile { path: path.clone(), source })?;
                spec.validate().map_err(|message| EvalFileError::InvalidInlineJudge { message })?;
                Ok(spec)
            }
        }
    }
}

impl<'de, T> Deserialize<'de> for PathOrInline<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(path) => Ok(Self::Path(PathBuf::from(path))),
            value => serde_json::from_value(value).map(Self::Inline).map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for DockerSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DockerSpec".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        DockerSpecRepr::json_schema(generator)
    }
}

impl<T: JsonSchema> JsonSchema for PathOrInline<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Owned(format!("PathOr{}", T::schema_name()))
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let inline = generator.subschema_for::<T>().to_value();
        schemars::Schema::try_from(serde_json::json!({
            "oneOf": [{ "type": "string" }, inline],
        }))
        .expect("PathOrInline schema must be valid")
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DockerSpecRepr {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    context: Option<PathBuf>,
}

impl TryFrom<DockerSpecRepr> for DockerSpec {
    type Error = &'static str;

    fn try_from(repr: DockerSpecRepr) -> Result<Self, Self::Error> {
        match (repr.file, repr.image, repr.context) {
            (Some(file), image, context) => Ok(Self::Build { file, image, context }),
            (None, Some(image), None) => Ok(Self::Prebuilt { image }),
            (None, Some(_), Some(_)) => Err("docker `context` requires a Dockerfile `file`"),
            (None, None, _) => Err("docker must specify a prebuilt `image` and/or a Dockerfile `file`"),
        }
    }
}

impl ToolCallExpectation {
    pub(crate) fn failure(&self, tool: &str, actual: usize) -> Option<String> {
        match self {
            Self::AtLeast(expected) if actual < *expected => Some(format!(
                "expected tool `{tool}` to be called at least {expected} time(s), but was called {actual}"
            )),
            Self::Exactly(expected) if actual != *expected => {
                Some(format!("expected tool `{tool}` to be called exactly {expected} time(s), but was called {actual}"))
            }
            _ => None,
        }
    }
}

impl JudgeSpec {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.criteria.is_empty() {
            return Err("judge criteria must not be empty".to_string());
        }

        let mut ids = BTreeSet::new();
        for criterion in &self.criteria {
            if criterion.id.trim().is_empty() {
                return Err("judge criterion id must not be empty".to_string());
            }
            if !ids.insert(criterion.id.clone()) {
                return Err(format!("duplicate judge criterion id `{}`", criterion.id));
            }
            if criterion.description.trim().is_empty() {
                return Err(format!("judge criterion `{}` description must not be empty", criterion.id));
            }
            if !criterion.weight.is_finite() || criterion.weight <= 0.0 {
                return Err(format!("judge criterion `{}` weight must be positive and finite", criterion.id));
            }
            if !criterion.threshold.is_finite() || !(0.0..=1.0).contains(&criterion.threshold) {
                return Err(format!("judge criterion `{}` threshold must be between 0.0 and 1.0", criterion.id));
            }
        }

        Ok(())
    }
}

fn canonical_docker_path(path: PathBuf) -> Result<PathBuf, EvalFileError> {
    path.canonicalize().map_err(|source| EvalFileError::DockerPath { path, source })
}

/// Tag for a built image whose eval file does not name one. Derived from the Dockerfile and
/// context paths so distinct builds never collide on a shared default tag, while eval files
/// sharing a Dockerfile and context share one build.
fn derived_tag(dockerfile: &Path, context: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    (dockerfile, context).hash(&mut hasher);
    format!("aether-eval-sandbox:{:016x}", hasher.finish())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_spec_parses_command_only_json() {
        let agent: AgentSpec = serde_json::from_str(
            r#"{"command":["node","/app/eval-agent.js"],"env":{"AETHER_EVAL_MODEL":"test:model"}}"#,
        )
        .unwrap();

        assert_eq!(agent.command, vec!["node", "/app/eval-agent.js"]);
        assert_eq!(agent.env["AETHER_EVAL_MODEL"], "test:model");
    }

    #[test]
    fn agent_spec_rejects_empty_command() {
        let agent: AgentSpec = serde_json::from_str(r#"{"command":[]}"#).unwrap();
        let error = agent.validate().unwrap_err();
        assert_eq!(error, "agent command must not be empty");
    }

    #[test]
    fn agent_spec_rejects_blank_program() {
        let agent: AgentSpec = serde_json::from_str(r#"{"command":["  "]}"#).unwrap();

        let error = agent.validate().unwrap_err();
        assert_eq!(error, "agent command program must not be empty");
    }

    #[test]
    fn agent_spec_rejects_invalid_env_keys() {
        for json in [r#"{"command":["agent"],"env":{"":"x"}}"#, r#"{"command":["agent"],"env":{"BAD=KEY":"x"}}"#] {
            let agent: AgentSpec = serde_json::from_str(json).unwrap();
            let error = agent.validate().unwrap_err();
            assert!(error.contains("agent env contains invalid key"), "got: {error}");
        }
    }

    #[test]
    fn agent_spec_rejects_old_tagged_shapes() {
        for json in [r#"{"type":"aether","settings":{}}"#, r#"{"type":"command","command":["agent"]}"#] {
            let error = serde_json::from_str::<AgentSpec>(json).unwrap_err().to_string();

            assert!(error.contains("unknown field `type`"), "got: {error}");
        }
    }

    #[test]
    fn judge_ref_parses_path_or_inline() {
        let path: JudgeRef = serde_json::from_str(r#""shared/maintainer.judge.json""#).unwrap();
        assert!(matches!(path, JudgeRef::Path(_)));

        let inline: JudgeRef =
            serde_json::from_str(r#"{"model":"m","criteria":[{"id":"a","description":"ok"}]}"#).unwrap();
        assert!(matches!(inline, JudgeRef::Inline(_)));
    }

    #[test]
    fn judge_ref_preserves_inline_validation_errors() {
        let error = serde_json::from_str::<JudgeRef>(r#"{"criteria":[{"id":"a","description":"ok"}]}"#).unwrap_err();

        assert!(error.to_string().contains("missing field `model`"), "got: {error}");
    }

    #[test]
    fn derived_tags_differ_per_dockerfile_and_match_for_identical_builds() {
        let tag_a = derived_tag(Path::new("/repo/a/Dockerfile"), Path::new("/repo"));
        let tag_b = derived_tag(Path::new("/repo/b/Dockerfile"), Path::new("/repo"));

        assert_eq!(tag_a, derived_tag(Path::new("/repo/a/Dockerfile"), Path::new("/repo")));
        assert_ne!(tag_a, tag_b);
    }
}
