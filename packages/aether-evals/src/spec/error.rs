use crate::agents::{DockerImageParseError, ImageBuildError};
use aether_project::SettingsError;
use std::path::PathBuf;
use thiserror::Error;

/// Eval-file setup errors. Setup — loading, image resolution and builds, settings — happens
/// before any eval runs and aborts the collection. Faults inside an individual eval (workspace
/// setup, the agent run, the judge call) do not abort a collection run; they are recorded on
/// that eval's [`crate::spec::EvalOutcome`] instead.
#[derive(Debug, Error)]
pub enum EvalFileError {
    #[error("failed to read eval file '{}': {source}", path.display())]
    ReadEvalFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse eval file '{}': {source}", path.display())]
    ParseEvalFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("eval path '{}' does not exist", path.display())]
    EvalPathNotFound { path: PathBuf },

    #[error("failed to discover eval files under '{}': {source}", path.display())]
    DiscoverEvalFiles {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("no eval files found under {}", format_paths(paths))]
    NoEvalFilesFound { paths: Vec<PathBuf> },

    #[error("no eval named '{name}' in the selected eval file(s)")]
    NoMatchingEval { name: String },

    #[error("duplicate eval name '{name}' in selected eval files")]
    DuplicateEvalName { name: String },

    #[error("failed to build judge model '{model}': {source}")]
    JudgeModel {
        model: String,
        #[source]
        source: llm::LlmError,
    },

    #[error("eval file does not specify a Docker image (set `docker.image` and/or `docker.file`)")]
    NoImage,

    #[error("invalid image reference: {0}")]
    Image(#[from] DockerImageParseError),

    #[error("failed to resolve docker path '{}': {source}", path.display())]
    DockerPath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    ImageBuild(#[from] ImageBuildError),

    #[error("invalid settings: {0}")]
    Settings(#[from] SettingsError),

    #[error("invalid inline judge: {message}")]
    InvalidInlineJudge { message: String },

    #[error("failed to read judge file '{}': {source}", path.display())]
    ReadJudgeFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse judge file '{}': {source}", path.display())]
    ParseJudgeFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

fn format_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}
