use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum McpServerStatus {
    Connected { tool_count: usize },
    Authenticating,
    Failed { error: String },
    NeedsOAuth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum McpServerAuthCapability {
    #[default]
    Unavailable,
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerStatusEntry {
    pub name: String,
    pub status: McpServerStatus,
    pub auth_capability: McpServerAuthCapability,
    #[serde(default)]
    pub proxied: bool,
}

impl McpServerStatusEntry {
    pub fn new(name: impl Into<String>, status: McpServerStatus) -> Self {
        Self { name: name.into(), status, auth_capability: McpServerAuthCapability::Unavailable, proxied: false }
    }

    pub fn with_auth_capability(mut self, auth: McpServerAuthCapability) -> Self {
        self.auth_capability = auth;
        self
    }

    pub fn with_proxied(mut self, proxied: bool) -> Self {
        self.proxied = proxied;
        self
    }

    pub fn can_authenticate(&self) -> bool {
        self.auth_capability == McpServerAuthCapability::OAuth
            && !matches!(self.status, McpServerStatus::Authenticating)
    }
}
