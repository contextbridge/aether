use crate::core::{AgentError, Result};
use glob::glob;
use mcp_utils::client::ServerInstructions;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tracing::warn;
use utils::shell_expander::ShellExpander;
use utils::substitution::substitute_parameters;

#[derive(Debug, Clone)]
pub enum Prompt {
    Text(String),
    File {
        path: String,
        args: Option<HashMap<String, String>>,
        cwd: Option<PathBuf>,
    },
    /// Resolve prompt files from glob patterns relative to cwd.
    /// Absolute paths are also supported.
    PromptGlobs {
        patterns: Vec<String>,
        cwd: PathBuf,
    },
    McpInstructions(Vec<ServerInstructions>),
}

/// Authored description of a prompt source — text, a file path, or a glob pattern.
///
/// Used by configuration layers to declare prompts before resolution. Convert into
/// runtime [`Prompt`] values with [`Prompt::from_sources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSource {
    Text { text: String },
    File { path: String },
    Glob { pattern: String },
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum PromptSourceInput {
    Path(String),
    Object(PromptSourceObject),
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum PromptSourceObject {
    Text { text: String },
    File { path: String },
    Glob { pattern: String },
}

impl<'de> Deserialize<'de> for PromptSource {
    fn deserialize<T: Deserializer<'de>>(deserializer: T) -> std::result::Result<Self, T::Error> {
        match serde::Deserialize::deserialize(deserializer)? {
            PromptSourceInput::Path(path) => Ok(Self::File { path }),
            PromptSourceInput::Object(PromptSourceObject::Text { text }) => Ok(Self::Text { text }),
            PromptSourceInput::Object(PromptSourceObject::File { path }) => Ok(Self::File { path }),
            PromptSourceInput::Object(PromptSourceObject::Glob { pattern }) => Ok(Self::Glob { pattern }),
        }
    }
}

impl Serialize for PromptSource {
    fn serialize<T: Serializer>(&self, serializer: T) -> std::result::Result<T::Ok, T::Error> {
        match self {
            Self::File { path } => serializer.serialize_str(path),
            Self::Text { text } => Serialize::serialize(&PromptSourceObject::Text { text: text.clone() }, serializer),
            Self::Glob { pattern } => {
                Serialize::serialize(&PromptSourceObject::Glob { pattern: pattern.clone() }, serializer)
            }
        }
    }
}

impl JsonSchema for PromptSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PromptSource".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let object_schema = generator.subschema_for::<PromptSourceObject>().to_value();
        Schema::try_from(serde_json::json!({
            "description": "Authored description of a prompt source — either a file path string or a typed text, file, or glob object.",
            "oneOf": [
                { "type": "string" },
                object_schema
            ]
        }))
        .expect("prompt source schema must be valid")
    }
}

impl PromptSource {
    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into() }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path } => Some(path.as_str()),
            Self::Glob { pattern } => Some(pattern.as_str()),
            Self::Text { .. } => None,
        }
    }
}

impl From<&str> for PromptSource {
    fn from(value: &str) -> Self {
        Self::file(value)
    }
}

impl From<String> for PromptSource {
    fn from(value: String) -> Self {
        Self::file(value)
    }
}

/// Validation failures raised while resolving [`PromptSource`] values into [`Prompt`]s.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PromptSourceError {
    /// A glob pattern is syntactically invalid.
    #[error("Invalid glob pattern '{pattern}': {error}")]
    InvalidGlobPattern { pattern: String, error: String },

    /// A prompt file or glob did not match any files on disk.
    #[error("Prompt entry '{pattern}' resolves to no files")]
    ZeroMatch { pattern: String },
}

impl Prompt {
    pub fn text(str: &str) -> Self {
        Self::Text(str.to_string())
    }

    pub fn file(path: &str) -> Self {
        Self::File { path: path.to_string(), args: None, cwd: None }
    }

    pub fn file_with_args(path: &str, args: HashMap<String, String>) -> Self {
        Self::File { path: path.to_string(), args: Some(args), cwd: None }
    }

    pub fn from_globs(patterns: Vec<String>, cwd: PathBuf) -> Self {
        Self::PromptGlobs { patterns, cwd }
    }

    /// Resolve a slice of [`PromptSource`] declarations into runtime [`Prompt`] values.
    ///
    /// Validates that file paths and glob patterns produce at least one matching file
    /// under `project_root`. Text sources pass through unchanged.
    pub fn from_sources(
        project_root: &Path,
        sources: &[PromptSource],
    ) -> std::result::Result<Vec<Prompt>, PromptSourceError> {
        sources
            .iter()
            .map(|source| match source {
                PromptSource::Text { text } => Ok(Prompt::text(text)),
                PromptSource::File { path } => validate_prompt_file(project_root, path)
                    .map(|()| Prompt::file(path).with_cwd(project_root.to_path_buf())),
                PromptSource::Glob { pattern } => validate_prompt_glob(project_root, pattern)
                    .map(|()| Prompt::from_globs(vec![pattern.clone()], project_root.to_path_buf())),
            })
            .collect()
    }

    pub fn with_cwd(self, cwd: PathBuf) -> Self {
        match self {
            Self::File { path, args, .. } => Self::File { path, args, cwd: Some(cwd) },
            Self::PromptGlobs { patterns, .. } => Self::PromptGlobs { patterns, cwd },
            Self::Text(_) | Self::McpInstructions(_) => self,
        }
    }

    pub fn mcp_instructions(instructions: Vec<ServerInstructions>) -> Self {
        Self::McpInstructions(instructions)
    }

    /// Resolve this `SystemPrompt` to a String
    pub async fn build(&self) -> Result<String> {
        match self {
            Prompt::Text(text) => Ok(text.clone()),
            Prompt::File { path, args, cwd } => {
                let content = Self::resolve_file(&PathBuf::from(path)).await?;
                let substituted = substitute_parameters(&content, args);
                let expander = ShellExpander::new();
                Self::expand_builtins(&substituted, cwd.as_deref(), &expander).await
            }
            Prompt::PromptGlobs { patterns, cwd } => Self::resolve_prompt_globs(patterns, cwd).await,
            Prompt::McpInstructions(instructions) => Ok(format_mcp_instructions(instructions)),
        }
    }

    /// Resolve multiple `SystemPrompts` and join them with double newlines
    pub async fn build_all(prompts: &[Prompt]) -> Result<String> {
        let mut parts = Vec::with_capacity(prompts.len());
        for p in prompts {
            let part = p.build().await?;
            if !part.is_empty() {
                parts.push(part);
            }
        }
        Ok(parts.join("\n\n"))
    }

    async fn resolve_file(path: &Path) -> Result<String> {
        fs::read_to_string(path)
            .await
            .map_err(|e| AgentError::IoError(format!("Failed to read file '{}': {e}", path.display())))
    }

    async fn resolve_prompt_globs(patterns: &[String], cwd: &Path) -> Result<String> {
        let mut contents = Vec::new();
        let expander = ShellExpander::new();

        for pattern in patterns {
            let full_pattern = if Path::new(pattern).is_absolute() {
                pattern.clone()
            } else {
                cwd.join(pattern).to_string_lossy().to_string()
            };

            let paths = glob(&full_pattern)
                .map_err(|e| AgentError::IoError(format!("Invalid glob pattern '{pattern}': {e}")))?;

            let mut matched: Vec<PathBuf> = paths.filter_map(std::result::Result::ok).collect();
            matched.sort();

            for path in matched {
                if path.is_file() {
                    match fs::read_to_string(&path).await {
                        Ok(content) => {
                            let resolved = Self::expand_builtins(&content, Some(cwd), &expander).await?;
                            contents.push(resolved);
                        }
                        Err(e) => {
                            warn!("Failed to read prompt file '{}': {e}", path.display());
                        }
                    }
                }
            }
        }

        Ok(contents.join("\n\n"))
    }

    /// Expand `` !`command` `` shell-interpolation markers in prompt content.
    ///
    /// Thin wrapper around [`ShellExpander::expand`] that resolves `cwd` from
    /// the process working directory when `None`.
    async fn expand_builtins(content: &str, cwd: Option<&Path>, expander: &ShellExpander) -> Result<String> {
        let cwd = match cwd {
            Some(dir) => dir.to_path_buf(),
            None => {
                env::current_dir().map_err(|e| AgentError::IoError(format!("Failed to get current directory: {e}")))?
            }
        };
        Ok(expander.expand(content, &cwd).await)
    }
}

fn validate_prompt_file(project_root: &Path, path: &str) -> std::result::Result<(), PromptSourceError> {
    let full_path = project_root.join(path);
    if full_path.is_file() { Ok(()) } else { Err(PromptSourceError::ZeroMatch { pattern: path.to_string() }) }
}

fn validate_prompt_glob(project_root: &Path, pattern: &str) -> std::result::Result<(), PromptSourceError> {
    let full_pattern = if Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        project_root.join(pattern).to_string_lossy().to_string()
    };

    let has_file_match = glob(&full_pattern)
        .map_err(|e| PromptSourceError::InvalidGlobPattern { pattern: pattern.to_string(), error: e.to_string() })?
        .filter_map(std::result::Result::ok)
        .any(|path| path.is_file());

    if has_file_match { Ok(()) } else { Err(PromptSourceError::ZeroMatch { pattern: pattern.to_string() }) }
}

/// Format MCP instructions with XML tags for the system prompt.
fn format_mcp_instructions(instructions: &[ServerInstructions]) -> String {
    if instructions.is_empty() {
        return String::new();
    }

    let mut parts = vec!["# MCP Server Instructions\n".to_string()];
    parts.push("You are connected to the following MCP servers:\n".to_string());

    for instr in instructions {
        parts.push(format!("<mcp-server name=\"{}\">\n{}\n</mcp-server>\n", instr.server_name, instr.instructions));
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_text_prompt() {
        let prompt = Prompt::text("Hello, world!");
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[tokio::test]
    async fn build_all_concatenates_prompts() {
        let prompts = vec![Prompt::text("Part one"), Prompt::text("Part two")];
        let result = Prompt::build_all(&prompts).await.unwrap();
        assert_eq!(result, "Part one\n\nPart two");
    }

    #[tokio::test]
    async fn prompt_globs_resolves_single_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Instructions\nBe helpful").unwrap();

        let prompt = Prompt::from_globs(vec!["AGENTS.md".to_string()], dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "# Instructions\nBe helpful");
    }

    #[tokio::test]
    async fn prompt_globs_resolves_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let rules_dir = dir.path().join(".aether/rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("a-coding.md"), "Use Rust").unwrap();
        std::fs::write(rules_dir.join("b-testing.md"), "Write tests").unwrap();

        let prompt = Prompt::from_globs(vec![".aether/rules/*.md".to_string()], dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.contains("Use Rust"));
        assert!(result.contains("Write tests"));
    }

    #[tokio::test]
    async fn prompt_globs_returns_empty_for_no_matches() {
        let dir = tempfile::tempdir().unwrap();

        let prompt = Prompt::from_globs(vec!["nonexistent*.md".to_string()], dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn prompt_globs_supports_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("rules.md");
        std::fs::write(&file_path, "Absolute rule").unwrap();

        let prompt = Prompt::from_globs(vec![file_path.to_string_lossy().to_string()], PathBuf::from("/tmp"));
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "Absolute rule");
    }

    #[tokio::test]
    async fn prompt_globs_concatenates_multiple_patterns() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Agent instructions").unwrap();
        std::fs::write(dir.path().join("SYSTEM.md"), "System prompt").unwrap();

        let prompt =
            Prompt::from_globs(vec!["AGENTS.md".to_string(), "SYSTEM.md".to_string()], dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.contains("Agent instructions"));
        assert!(result.contains("System prompt"));
        assert!(result.contains("\n\n"));
    }

    #[tokio::test]
    async fn build_all_skips_empty_parts() {
        let prompts = vec![Prompt::text("Part one"), Prompt::text(""), Prompt::text("Part two")];
        let result = Prompt::build_all(&prompts).await.unwrap();
        assert_eq!(result, "Part one\n\nPart two");
    }

    #[tokio::test]
    async fn expand_builtins_no_op_without_marker() {
        let content = "Just some plain content with no directives";
        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins(content, None, &expander).await.unwrap();
        assert_eq!(result, content);
    }

    #[tokio::test]
    async fn expand_builtins_runs_shell_command() {
        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins("branch: !`echo main`", None, &expander).await.unwrap();
        assert_eq!(result, "branch: main");
    }

    #[tokio::test]
    async fn expand_builtins_runs_command_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sentinel.txt"), "").unwrap();

        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins("files: !`ls`", Some(dir.path()), &expander).await.unwrap();
        assert!(result.contains("sentinel.txt"), "expected sentinel.txt in output: {result}");
    }

    #[tokio::test]
    async fn expand_builtins_handles_multiple_commands() {
        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins("a=!`echo one`, b=!`echo two`", None, &expander).await.unwrap();
        assert_eq!(result, "a=one, b=two");
    }

    #[tokio::test]
    async fn expand_builtins_substitutes_empty_on_failure() {
        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins("before !`exit 1` after", None, &expander).await.unwrap();
        assert_eq!(result, "before  after");
    }

    #[tokio::test]
    async fn expand_builtins_trims_trailing_whitespace() {
        let expander = ShellExpander::new();
        let result = Prompt::expand_builtins("!`printf 'hi\\n\\n'`", None, &expander).await.unwrap();
        assert_eq!(result, "hi");
    }

    #[tokio::test]
    async fn prompt_globs_expands_shell_in_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Instructions\n\nbranch: !`echo main`\n\nRules").unwrap();

        let prompt = Prompt::from_globs(vec!["AGENTS.md".to_string()], dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.contains("Instructions"));
        assert!(result.contains("branch: main"));
        assert!(result.contains("Rules"));
        assert!(!result.contains("!`"));
    }
}
