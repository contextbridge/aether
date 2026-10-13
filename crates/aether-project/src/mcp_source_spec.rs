use crate::SettingsError;
use aether_core::mcp::{McpConfig, McpServerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use utils::variables::VarError;
use utils::{PathOrObject, ResourcePath};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "PathOrObject<McpSourceSpecObject>", into = "PathOrObject<McpSourceSpecObject>")]
#[schemars(with = "PathOrObject<McpSourceSpecObject>", description = include_str!("docs/mcp_source_spec.md"))]
pub enum McpSourceSpec {
    File(McpFileSpec),
    Inline(McpConfig),
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpFileSpec {
    pub path: ResourcePath,
    pub defer_tools: bool,
    pub optional: bool,
}

impl McpFileSpec {
    pub fn new(path: impl Into<ResourcePath>) -> Self {
        Self { path: path.into(), defer_tools: false, optional: false }
    }

    #[must_use]
    pub fn defer_tools(mut self) -> Self {
        self.defer_tools = true;
        self
    }

    #[must_use]
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub fn load(&self, root: &Path) -> Result<Option<McpConfig>, SettingsError> {
        let Some(path) = self.resolve(root)? else { return Ok(None) };
        let mut config = McpConfig::from_json_file(&path)
            .map_err(|source| SettingsError::McpConfig { path: self.path.as_authored().to_string(), source })?;
        if self.defer_tools {
            config.defer_all_tools();
        }
        Ok(Some(config))
    }

    fn resolve(&self, root: &Path) -> Result<Option<PathBuf>, SettingsError> {
        let full_path = match self.path.resolve(root) {
            Ok(full_path) => full_path,
            Err(VarError::NotFound(variable)) if self.optional => {
                tracing::warn!(
                    "Skipping optional MCP config '{}': variable '{variable}' is not defined",
                    self.path.as_authored()
                );
                return Ok(None);
            }
            Err(VarError::NotFound(variable)) => {
                return Err(SettingsError::UnresolvedMcpConfigVariable {
                    path: self.path.as_authored().to_string(),
                    variable,
                });
            }
        };

        if full_path.is_file() {
            Ok(Some(full_path))
        } else if self.optional {
            Ok(None)
        } else {
            Err(SettingsError::InvalidMcpConfigPath { path: self.path.as_authored().to_string() })
        }
    }
}

impl McpSourceSpec {
    pub fn file(path: impl Into<ResourcePath>) -> Self {
        Self::File(McpFileSpec::new(path))
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File(file) => Some(file.path.as_authored()),
            Self::Inline(_) => None,
        }
    }

    pub fn load(&self, root: &Path) -> Result<Option<McpConfig>, SettingsError> {
        match self {
            Self::File(file) => file.load(root),
            Self::Inline(config) => Ok(Some(config.clone())),
        }
    }
}

impl From<McpFileSpec> for McpSourceSpec {
    fn from(spec: McpFileSpec) -> Self {
        Self::File(spec)
    }
}

impl From<PathOrObject<McpSourceSpecObject>> for McpSourceSpec {
    fn from(source: PathOrObject<McpSourceSpecObject>) -> Self {
        match source {
            PathOrObject::Path(path) => Self::file(path),
            PathOrObject::Object(McpSourceSpecObject::File { path, defer_tools, optional }) => {
                Self::File(McpFileSpec { path, defer_tools, optional })
            }
            PathOrObject::Object(McpSourceSpecObject::Inline { servers }) => Self::Inline(McpConfig::new(servers)),
        }
    }
}

impl From<McpSourceSpec> for PathOrObject<McpSourceSpecObject> {
    fn from(source: McpSourceSpec) -> Self {
        match source {
            McpSourceSpec::File(McpFileSpec { path, defer_tools: false, optional: false }) => Self::Path(path),
            McpSourceSpec::File(McpFileSpec { path, defer_tools, optional }) => {
                Self::Object(McpSourceSpecObject::File { path, defer_tools, optional })
            }
            McpSourceSpec::Inline(config) => Self::Object(McpSourceSpecObject::Inline { servers: config.servers }),
        }
    }
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum McpSourceSpecObject {
    File {
        path: ResourcePath,
        #[serde(rename = "deferTools", default, skip_serializing_if = "std::ops::Not::not")]
        defer_tools: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
    },
    Inline {
        servers: BTreeMap<String, McpServerConfig>,
    },
}
