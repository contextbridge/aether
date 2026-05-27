use crate::variables::{VarError, Vars};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum PathOrObject<O> {
    Path(ResourcePath),
    Object(O),
}

pub fn string_or_object_schema(description: &str, object_schema: &serde_json::Value) -> Schema {
    Schema::try_from(serde_json::json!({
        "description": description,
        "oneOf": [{ "type": "string" }, object_schema],
    }))
    .expect("string_or_object schema must be valid")
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
        let s = self.0.as_str();
        let vars = Vars::new().with("WORKSPACE", workspace_root.to_string_lossy().into_owned());
        if vars.has_reference(s) {
            let expanded = vars.expand(s)?;
            let path = PathBuf::from(&expanded);
            Ok(if path.is_absolute() { path } else { workspace_root.join(path) })
        } else if Path::new(s).is_absolute() {
            Ok(PathBuf::from(s))
        } else {
            Ok(workspace_root.join(s))
        }
    }

    /// Convert a bare relative path to an absolute one in place.
    pub fn promote_relative(&mut self, source_root: &Path) {
        let s = self.0.as_str();
        if !Vars::new().has_reference(s) && !Path::new(s).is_absolute() {
            self.0 = source_root.join(s).to_string_lossy().into_owned();
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
    fn resolve_expands_aether_workspace_token() {
        let root = PathBuf::from("/workspace");
        let path: ResourcePath = "${WORKSPACE}/AGENTS.md".into();
        assert_eq!(path.resolve(&root).unwrap(), PathBuf::from("/workspace/AGENTS.md"));
    }

    #[test]
    fn resolve_joins_relative_with_workspace() {
        let root = PathBuf::from("/workspace");
        let path: ResourcePath = "AGENTS.md".into();
        assert_eq!(path.resolve(&root).unwrap(), PathBuf::from("/workspace/AGENTS.md"));
    }

    #[test]
    fn resolve_keeps_absolute_path() {
        let root = PathBuf::from("/workspace");
        let path: ResourcePath = "/abs/AGENTS.md".into();
        assert_eq!(path.resolve(&root).unwrap(), PathBuf::from("/abs/AGENTS.md"));
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
