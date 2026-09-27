use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum McpServerStatus {
    Connecting,
    Connected {
        tool_count: usize,
    },
    Authenticating,
    Failed {
        error: String,
    },
    #[serde(rename = "needs_oauth")]
    NeedsOAuth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum McpServerAuthCapability {
    #[default]
    Unavailable,
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusEntry {
    pub name: String,
    pub status: McpServerStatus,
    pub auth_capability: McpServerAuthCapability,
    #[serde(rename = "deferTools", default)]
    pub deferred_tools: bool,
}

impl McpServerStatusEntry {
    pub fn new(name: impl Into<String>, status: McpServerStatus) -> Self {
        Self { name: name.into(), status, auth_capability: McpServerAuthCapability::Unavailable, deferred_tools: false }
    }

    pub fn with_auth_capability(mut self, auth: McpServerAuthCapability) -> Self {
        self.auth_capability = auth;
        self
    }

    pub fn with_deferred_tools(mut self, deferred_tools: bool) -> Self {
        self.deferred_tools = deferred_tools;
        self
    }

    pub fn can_authenticate(&self) -> bool {
        self.auth_capability == McpServerAuthCapability::OAuth
    }
}
