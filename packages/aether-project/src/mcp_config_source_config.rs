use mcp_utils::client::RawMcpServerConfig;
use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum McpSourceSpec {
    File {
        path: String,
        #[serde(default)]
        proxy: bool,
    },
    Inline {
        servers: BTreeMap<String, RawMcpServerConfig>,
    },
}

impl McpSourceSpec {
    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into(), proxy: false }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path, .. } => Some(path.as_str()),
            Self::Inline { .. } => None,
        }
    }
}
