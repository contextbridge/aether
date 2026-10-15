use crate::variables::{VarError, Vars};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PathOrObject<O> {
    Path(ResourcePath),
    Object(O),
}

/// A path declared in user-facing settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourcePath(String);

impl ResourcePath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The user-visible form of the path, suitable for round-trip serialization
    /// and for inclusion in error messages.
    pub fn as_authored(&self) -> &str {
        &self.0
    }

    /// Expand this path to an absolute [`PathBuf`].
    pub fn resolve(&self, workspace_root: &Path) -> Result<PathBuf, VarError> {
        let vars = Vars::new().with("WORKSPACE", workspace_root.to_string_lossy().into_owned());
        Ok(workspace_root.join(vars.expand(&self.0)?))
    }

    /// Convert a bare relative path to an absolute one in place.
    pub fn promote_relative(&mut self, source_root: &Path) {
        if !Vars::has_reference(&self.0) && !Path::new(&self.0).is_absolute() {
            self.0 = source_root.join(&self.0).to_string_lossy().into_owned();
        }
    }
}

impl From<&str> for ResourcePath {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ResourcePath {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl JsonSchema for ResourcePath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ResourcePath".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "A path with optional `$VAR` / `${VAR}` expansion. `${WORKSPACE}` resolves to the workspace root; other names fall through to process env. Plain relative paths resolve against the workspace root at use time."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_string_verbatim() {
        let cases = ["${WORKSPACE}/AGENTS.md", "AGENTS.md", "/abs/AGENTS.md", "nested/dir/file.md", "$HOME/scratch"];
        for input in cases {
            let path: ResourcePath = input.into();
            assert_eq!(path.as_authored(), input, "round-trip failed for {input}");
            let json = serde_json::to_string(&path).unwrap();
            assert_eq!(json, format!("\"{input}\""));
            let parsed: ResourcePath = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, path);
        }
    }

    #[test]
    fn resolve_expands_variables_against_the_workspace_root() {
        let root = PathBuf::from("/workspace");
        for (input, expected) in [
            ("${WORKSPACE}/AGENTS.md", "/workspace/AGENTS.md"),
            ("$WORKSPACE/a", "/workspace/a"),
            ("AGENTS.md", "/workspace/AGENTS.md"),
            ("/abs/AGENTS.md", "/abs/AGENTS.md"),
            ("$$x/y", "/workspace/$x/y"),
        ] {
            assert_eq!(ResourcePath::from(input).resolve(&root).unwrap(), PathBuf::from(expected), "{input}");
        }
    }

    #[test]
    fn resolve_reports_missing_variable() {
        let root = PathBuf::from("/workspace");
        let path: ResourcePath = "${DEFINITELY_NOT_SET_VAR}/foo".into();
        assert!(matches!(path.resolve(&root), Err(VarError::NotFound(name)) if name == "DEFINITELY_NOT_SET_VAR"));
    }

    #[test]
    fn promote_relative_joins_plain_relative_paths() {
        let source_root = PathBuf::from("/user/.aether");
        let mut path: ResourcePath = "agents/foo.md".into();
        path.promote_relative(&source_root);
        assert_eq!(path, ResourcePath::from("/user/.aether/agents/foo.md"));
    }

    #[test]
    fn promote_relative_treats_escaped_dollar_paths_as_relative() {
        let source_root = PathBuf::from("/user/.aether");
        let mut path: ResourcePath = "$$x/y".into();
        path.promote_relative(&source_root);
        assert_eq!(path.resolve(Path::new("/workspace")).unwrap(), PathBuf::from("/user/.aether/$x/y"));
    }

    #[test]
    fn promote_relative_leaves_variable_paths_untouched() {
        let source_root = PathBuf::from("/user/.aether");
        let mut path: ResourcePath = "${WORKSPACE}/AGENTS.md".into();
        let original = path.clone();
        path.promote_relative(&source_root);
        assert_eq!(path, original);
    }

    #[test]
    fn promote_relative_leaves_absolute_paths_untouched() {
        let source_root = PathBuf::from("/user/.aether");
        let mut path: ResourcePath = "/abs/foo.md".into();
        let original = path.clone();
        path.promote_relative(&source_root);
        assert_eq!(path, original);
    }
}
