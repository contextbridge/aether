use crate::core::{AgentError, Result};
use glob::glob;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tracing::warn;
use utils::shell_expander::ShellExpander;
use utils::substitution::substitute_parameters;
use utils::variables::VarError;
use utils::{PathOrObject, ResourcePath, is_false, string_or_object_schema};

#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    Text(String),
    File {
        path: PathBuf,
        args: Option<HashMap<String, String>>,
        cwd: PathBuf,
    },
    /// MCP server instructions keyed by server name. `BTreeMap` gives a
    /// stable render order, which is useful for prompt-caching
    McpInstructions(BTreeMap<String, String>),
}

/// Authored description of a prompt source — text, a file path, or a glob pattern.
///
/// Used by configuration layers to declare prompts before resolution. Convert into
/// runtime [`Prompt`] values with [`Prompt::from_sources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSource {
    Text { text: String },
    File { path: ResourcePath, optional: bool },
    Glob { pattern: ResourcePath, optional: bool },
}

impl PromptSource {
    pub fn file(path: impl Into<ResourcePath>) -> Self {
        Self::File { path: path.into(), optional: false }
    }

    pub fn glob(pattern: impl Into<ResourcePath>) -> Self {
        Self::Glob { pattern: pattern.into(), optional: false }
    }

    #[must_use]
    pub fn optional(self) -> Self {
        match self {
            Self::File { path, .. } => Self::File { path, optional: true },
            Self::Glob { pattern, .. } => Self::Glob { pattern, optional: true },
            Self::Text { .. } => self,
        }
    }

    /// The authored path/pattern string, if this source has one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path, .. } => Some(path.as_authored()),
            Self::Glob { pattern, .. } => Some(pattern.as_authored()),
            Self::Text { .. } => None,
        }
    }

    pub fn is_optional(&self) -> bool {
        match self {
            Self::File { optional, .. } | Self::Glob { optional, .. } => *optional,
            Self::Text { .. } => false,
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

impl From<PromptSourceObject> for PromptSource {
    fn from(object: PromptSourceObject) -> Self {
        match object {
            PromptSourceObject::Text { text } => Self::Text { text },
            PromptSourceObject::File { path, optional } => Self::File { path, optional },
            PromptSourceObject::Glob { pattern, optional } => Self::Glob { pattern, optional },
        }
    }
}

impl<'de> Deserialize<'de> for PromptSource {
    fn deserialize<T: Deserializer<'de>>(deserializer: T) -> std::result::Result<Self, T::Error> {
        Ok(match PathOrObject::<PromptSourceObject>::deserialize(deserializer)? {
            PathOrObject::Path(path) => Self::File { path, optional: false },
            PathOrObject::Object(object) => object.into(),
        })
    }
}

impl Serialize for PromptSource {
    fn serialize<T: Serializer>(&self, serializer: T) -> std::result::Result<T::Ok, T::Error> {
        match self {
            Self::File { path, optional: false } => path.serialize(serializer),
            Self::File { path, optional } => {
                Serialize::serialize(&PromptSourceObject::File { path: path.clone(), optional: *optional }, serializer)
            }
            Self::Text { text } => Serialize::serialize(&PromptSourceObject::Text { text: text.clone() }, serializer),
            Self::Glob { pattern, optional } => Serialize::serialize(
                &PromptSourceObject::Glob { pattern: pattern.clone(), optional: *optional },
                serializer,
            ),
        }
    }
}

impl JsonSchema for PromptSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PromptSource".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        string_or_object_schema(
            include_str!("../docs/prompt_source.md"),
            &generator.subschema_for::<PromptSourceObject>().to_value(),
        )
    }
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum PromptSourceObject {
    Text {
        /// Literal prompt text included verbatim.
        text: String,
    },
    File {
        /// Path to a prompt file, resolved as a resource path.
        path: ResourcePath,
        /// When true, a missing file is skipped instead of raising an error.
        #[serde(default, skip_serializing_if = "is_false")]
        optional: bool,
    },
    Glob {
        /// Glob pattern matching prompt files, resolved as a resource path.
        pattern: ResourcePath,
        /// When true, a zero-match glob is skipped instead of raising an error.
        #[serde(default, skip_serializing_if = "is_false")]
        optional: bool,
    },
}

/// Errors raised while resolving [`PromptSource`] values into [`Prompt`]s.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PromptSourceError {
    /// A glob pattern is syntactically invalid.
    #[error("Invalid glob pattern '{pattern}': {error}")]
    InvalidGlobPattern { pattern: String, error: String },

    /// A prompt file does not exist on disk.
    #[error("Prompt file '{path}' does not exist")]
    Missing { path: String },

    /// A prompt glob matched no files.
    #[error("Prompt glob '{pattern}' matched no files")]
    ZeroMatch { pattern: String },

    /// A `${VAR}` reference in a prompt path could not be resolved.
    #[error("Prompt entry '{pattern}' references undefined variable '{variable}'")]
    UnresolvedVariable { pattern: String, variable: String },
}

impl Prompt {
    pub fn text(str: &str) -> Self {
        Self::Text(str.to_string())
    }

    pub fn file(path: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into(), args: None, cwd: cwd.into() }
    }

    /// Resolve a slice of [`PromptSource`] declarations into runtime [`Prompt`] values.
    pub fn from_sources(
        workspace_root: &Path,
        sources: &[PromptSource],
    ) -> std::result::Result<Vec<Prompt>, PromptSourceError> {
        let mut prompts = Vec::new();
        for source in sources {
            if let PromptSource::Text { text } = source {
                prompts.push(Prompt::text(text));
                continue;
            }
            match resolve_source_files(workspace_root, source) {
                Ok(paths) => {
                    for path in paths {
                        prompts.push(Prompt::file(path, workspace_root.to_path_buf()));
                    }
                }
                Err(PromptSourceError::Missing { .. }) if source.is_optional() => {}
                Err(PromptSourceError::UnresolvedVariable { variable, .. }) if source.is_optional() => {
                    warn!(
                        "Skipping optional prompt entry '{}': variable '{variable}' is not defined",
                        source.path().unwrap_or_default()
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Ok(prompts)
    }

    /// Resolve this `SystemPrompt` to a String
    pub async fn build(&self) -> Result<String> {
        match self {
            Prompt::Text(text) => Ok(text.clone()),
            Prompt::File { path, args, cwd } => {
                let content = Self::resolve_file(path).await?;
                let substituted = substitute_parameters(&content, args);
                let expander = ShellExpander::new();
                Ok(expander.expand(&substituted, cwd).await)
            }
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
}

fn resolve_source_files(
    workspace_root: &Path,
    source: &PromptSource,
) -> std::result::Result<Vec<PathBuf>, PromptSourceError> {
    match source {
        PromptSource::Text { .. } => Ok(Vec::new()),
        PromptSource::File { path, .. } => {
            let full_path = resolve_path(path, workspace_root)?;
            if full_path.is_file() {
                Ok(vec![full_path])
            } else {
                Err(PromptSourceError::Missing { path: path.as_authored().to_string() })
            }
        }
        PromptSource::Glob { pattern, optional } => {
            let full_pattern = resolve_path(pattern, workspace_root)?;
            let mut paths: Vec<PathBuf> = glob(&full_pattern.to_string_lossy())
                .map_err(|e| PromptSourceError::InvalidGlobPattern {
                    pattern: pattern.as_authored().to_string(),
                    error: e.to_string(),
                })?
                .filter_map(std::result::Result::ok)
                .filter(|path| path.is_file())
                .collect();
            paths.sort();
            if paths.is_empty() && !*optional {
                Err(PromptSourceError::ZeroMatch { pattern: pattern.as_authored().to_string() })
            } else {
                Ok(paths)
            }
        }
    }
}

fn resolve_path(path: &ResourcePath, workspace_root: &Path) -> std::result::Result<PathBuf, PromptSourceError> {
    path.resolve(workspace_root).map_err(|VarError::NotFound(variable)| PromptSourceError::UnresolvedVariable {
        pattern: path.as_authored().to_string(),
        variable,
    })
}

pub struct PromptCache {
    prompts: Vec<Prompt>,
    entries: Vec<(Prompt, String)>,
}

impl PromptCache {
    pub fn new(mut prompts: Vec<Prompt>) -> Self {
        if !prompts.iter().any(|p| matches!(p, Prompt::McpInstructions(_))) {
            prompts.push(Prompt::McpInstructions(BTreeMap::new()));
        }
        Self { prompts, entries: Vec::new() }
    }

    pub fn update_mcp_instruction(&mut self, server: String, body: Option<String>) {
        for prompt in &mut self.prompts {
            if let Prompt::McpInstructions(map) = prompt {
                match body {
                    Some(text) => {
                        map.insert(server, text);
                    }
                    None => {
                        map.remove(&server);
                    }
                }
                return;
            }
        }
    }

    pub async fn render(&mut self) -> Result<String> {
        self.entries.truncate(self.prompts.len());
        let mut rendered_prompt = String::new();
        for i in 0..self.prompts.len() {
            let prompt = &self.prompts[i];
            match self.entries.get_mut(i) {
                Some((cached, _)) if *cached == *prompt => {}
                Some(entry) => *entry = (prompt.clone(), prompt.build().await?),
                None => self.entries.push((prompt.clone(), prompt.build().await?)),
            }

            let (_, body) = &self.entries[i];
            if !body.is_empty() {
                if !rendered_prompt.is_empty() {
                    rendered_prompt.push_str("\n\n");
                }
                rendered_prompt.push_str(body);
            }
        }
        Ok(rendered_prompt)
    }
}

/// Format MCP instructions with XML tags for the system prompt.
fn format_mcp_instructions(instructions: &BTreeMap<String, String>) -> String {
    if instructions.is_empty() {
        return String::new();
    }

    let mut parts = vec!["# MCP Server Instructions\n".to_string()];
    parts.push("You are connected to the following MCP servers:\n".to_string());

    for (server_name, body) in instructions {
        parts.push(format!("<mcp-server name=\"{server_name}\">\n{body}\n</mcp-server>\n"));
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use std::fs::{create_dir_all, write};

    use super::*;
    use crate::testing::mcp_instructions as instructions;

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
    async fn build_all_concatenates_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Agent instructions").unwrap();
        std::fs::write(dir.path().join("SYSTEM.md"), "System prompt").unwrap();

        let prompts = vec![
            Prompt::file(dir.path().join("AGENTS.md"), dir.path()),
            Prompt::file(dir.path().join("SYSTEM.md"), dir.path()),
        ];
        let result = Prompt::build_all(&prompts).await.unwrap();
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
    async fn prompt_cache_render_matches_build_all_on_first_render() {
        let prompts = vec![
            Prompt::text("first"),
            Prompt::McpInstructions(instructions(&[("srv", "body")])),
            Prompt::text("last"),
        ];
        let expected = Prompt::build_all(&prompts).await.unwrap();
        let mut cache = PromptCache::new(prompts);
        assert_eq!(cache.render().await.unwrap(), expected);
    }

    #[tokio::test]
    async fn prompt_cache_reuses_unchanged_slots() {
        use std::fs::{remove_file, write};

        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("AGENTS.md"), "cached body").unwrap();
        let mut cache = PromptCache::new(vec![
            Prompt::file(dir.path().join("AGENTS.md"), dir.path()),
            Prompt::McpInstructions(BTreeMap::new()),
        ]);

        cache.render().await.unwrap();

        // Remove the source file to prove we cached things
        remove_file(dir.path().join("AGENTS.md")).unwrap();
        cache.update_mcp_instruction("srv".into(), Some("instr".into()));

        let rendered = cache.render().await.unwrap();
        assert!(rendered.contains("cached body"));
        assert!(rendered.contains("instr"));
    }

    #[tokio::test]
    async fn prompt_cache_empty_renders_empty() {
        assert_eq!(PromptCache::new(vec![]).render().await.unwrap(), "");
    }

    #[tokio::test]
    async fn prompt_cache_drops_empty_slots() {
        let mut cache = PromptCache::new(vec![Prompt::text("a"), Prompt::text("b")]);
        assert_eq!(cache.render().await.unwrap(), "a\n\nb");
    }

    #[tokio::test]
    async fn build_file_expands_shell_commands() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("AGENTS.md"), "Instructions\n\nbranch: !`echo main`\n\nRules").unwrap();

        let prompt = Prompt::file(dir.path().join("AGENTS.md"), dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.contains("Instructions"));
        assert!(result.contains("branch: main"));
        assert!(result.contains("Rules"));
        assert!(!result.contains("!`"));
    }

    #[tokio::test]
    async fn build_file_runs_shell_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("sentinel.txt"), "").unwrap();
        let prompt_path = dir.path().join("AGENTS.md");
        write(&prompt_path, "files: !`ls`").unwrap();

        let prompt = Prompt::file(prompt_path, dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert!(result.contains("sentinel.txt"), "expected sentinel.txt in output: {result}");
    }

    #[tokio::test]
    async fn build_file_handles_multiple_commands() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("AGENTS.md");
        write(&prompt_path, "a=!`echo one`, b=!`echo two`").unwrap();

        let prompt = Prompt::file(prompt_path, dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "a=one, b=two");
    }

    #[tokio::test]
    async fn build_file_substitutes_empty_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("AGENTS.md");
        write(&prompt_path, "before !`exit 1` after").unwrap();

        let prompt = Prompt::file(prompt_path, dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "before  after");
    }

    #[tokio::test]
    async fn build_file_trims_trailing_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("AGENTS.md");
        write(&prompt_path, "!`printf 'hi\\n\\n'`").unwrap();

        let prompt = Prompt::file(prompt_path, dir.path().to_path_buf());
        let result = prompt.build().await.unwrap();
        assert_eq!(result, "hi");
    }

    #[test]
    fn optional_file_source_skips_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("EXISTS.md"), "exists").unwrap();

        let sources = vec![PromptSource::file("EXISTS.md"), PromptSource::file("MISSING.md").optional()];
        let prompts = Prompt::from_sources(dir.path(), &sources).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn optional_glob_source_skips_zero_matches() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("EXISTS.md"), "exists").unwrap();

        let sources = vec![PromptSource::file("EXISTS.md"), PromptSource::glob("nonexistent*.md").optional()];
        let prompts = Prompt::from_sources(dir.path(), &sources).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn required_glob_source_expands_to_one_prompt_per_match() {
        let dir = tempfile::tempdir().unwrap();
        let rules_dir = dir.path().join(".aether/rules");
        create_dir_all(&rules_dir).unwrap();
        write(rules_dir.join("a.md"), "a").unwrap();
        write(rules_dir.join("b.md"), "b").unwrap();

        let sources = vec![PromptSource::glob(".aether/rules/*.md")];
        let prompts = Prompt::from_sources(dir.path(), &sources).unwrap();
        assert_eq!(prompts.len(), 2);
    }

    #[test]
    fn required_glob_source_with_no_matches_errors() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![PromptSource::glob("nonexistent*.md")];
        let err = Prompt::from_sources(dir.path(), &sources).unwrap_err();
        assert!(matches!(err, PromptSourceError::ZeroMatch { .. }));
    }

    #[test]
    fn optional_glob_source_still_errors_on_invalid_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![PromptSource::glob("[invalid").optional()];
        let err = Prompt::from_sources(dir.path(), &sources).unwrap_err();
        assert!(matches!(err, PromptSourceError::InvalidGlobPattern { .. }));
    }

    #[test]
    fn optional_file_source_skips_unresolved_variable() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("EXISTS.md"), "exists").unwrap();

        let sources = vec![
            PromptSource::file("EXISTS.md"),
            PromptSource::file("${DEFINITELY_NOT_SET_VAR_PROMPT_FILE}/foo.md").optional(),
        ];
        let prompts = Prompt::from_sources(dir.path(), &sources).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn optional_glob_source_skips_unresolved_variable() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("EXISTS.md"), "exists").unwrap();

        let sources = vec![
            PromptSource::file("EXISTS.md"),
            PromptSource::glob("${DEFINITELY_NOT_SET_VAR_PROMPT_GLOB}/*.md").optional(),
        ];
        let prompts = Prompt::from_sources(dir.path(), &sources).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn required_file_source_errors_on_unresolved_variable() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![PromptSource::file("${DEFINITELY_NOT_SET_VAR_PROMPT_REQ}/foo.md")];
        let err = Prompt::from_sources(dir.path(), &sources).unwrap_err();
        assert!(matches!(err, PromptSourceError::UnresolvedVariable { .. }));
    }

    #[test]
    fn prompt_source_string_shorthand_is_required_file() {
        let source: PromptSource = serde_json::from_str(r#""SYSTEM.md""#).unwrap();
        assert_eq!(source, PromptSource::file("SYSTEM.md"));
    }

    #[test]
    fn optional_prompt_source_serializes_as_typed_object() {
        let source = PromptSource::file("${WORKSPACE}/AGENTS.md").optional();
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(value, serde_json::json!({"type":"file","path":"${WORKSPACE}/AGENTS.md","optional":true}));

        // Non-optional file stays as string shorthand
        let source = PromptSource::file("SYSTEM.md");
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(value, serde_json::json!("SYSTEM.md"));
    }

    #[test]
    fn optional_prompt_source_deserializes_from_typed_object() {
        let source: PromptSource =
            serde_json::from_str(r#"{"type":"file","path":"${WORKSPACE}/AGENTS.md","optional":true}"#).unwrap();
        assert_eq!(source, PromptSource::file("${WORKSPACE}/AGENTS.md").optional());
    }

    #[test]
    fn optional_glob_source_deserializes_from_typed_object() {
        let source: PromptSource =
            serde_json::from_str(r#"{"type":"glob","pattern":"${WORKSPACE}/.aether/rules/*.md","optional":true}"#)
                .unwrap();
        assert_eq!(source, PromptSource::glob("${WORKSPACE}/.aether/rules/*.md").optional());
    }
}
