use crate::{tool_exposure::ToolExposure, tool_policy::ToolFilter};
use rmcp::model::{MetaObject, Tool};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TOOL_CONTEXT_KEY: &str = "aether-agent.io/tool-context";
pub const AETHER_MCP_REQUEST_CONTEXT: &str = "AETHER_MCP_REQUEST_CONTEXT";

/// Execution identity, created once per harness MCP runtime, not per connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentIdentity {
    pub runtime_id: Uuid,
}

/// Trusted harness policy. This metadata is not an authentication credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayRequestContext {
    pub identity: AgentIdentity,
    pub server_alias: String,
    pub agent_tools: ToolFilter,
    pub server_tools: ToolFilter,
    pub defer_tools: ToolExposure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_task: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RequestContextError {
    #[error("missing Aether tool context")]
    Missing,
    #[error("invalid Aether tool context")]
    Invalid,
    #[error("server aliases must be nonempty and cannot contain '__'")]
    InvalidAlias,
}

impl AgentIdentity {
    pub fn new() -> Self {
        Self { runtime_id: Uuid::new_v4() }
    }
}

impl Default for AgentIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayRequestContext {
    pub fn validate(&self) -> Result<(), RequestContextError> {
        if self.identity.runtime_id.is_nil() {
            return Err(RequestContextError::Invalid);
        }
        validate_server_alias(&self.server_alias)
    }

    pub fn from_json(json: &str) -> Result<Self, RequestContextError> {
        let context: Self = serde_json::from_str(json).map_err(|_| RequestContextError::Invalid)?;
        context.validate()?;
        Ok(context)
    }

    pub fn from_meta(meta: Option<&MetaObject>) -> Result<Self, RequestContextError> {
        let value = meta.and_then(|meta| meta.get(TOOL_CONTEXT_KEY)).ok_or(RequestContextError::Missing)?;
        let context: Self = serde_json::from_value(value.clone()).map_err(|_| RequestContextError::Invalid)?;
        context.validate()?;
        Ok(context)
    }

    /// Replace only the reserved context, retaining tracing and protocol metadata.
    pub fn merge_into(&self, meta: &mut MetaObject) -> Result<(), RequestContextError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| RequestContextError::Invalid)?;
        meta.insert(TOOL_CONTEXT_KEY.to_owned(), value);
        Ok(())
    }

    /// Intersect independent policies in their respective naming domains.
    pub fn allows(&self, backend: &str, deployment: &ToolFilter, tool: &Tool) -> bool {
        if self.validate().is_err() || validate_server_alias(backend).is_err() {
            return false;
        }
        let aggregate_name = format!("{backend}__{}", tool.name);
        let model_name = format!("{}__{aggregate_name}", self.server_alias);
        deployment.is_tool_allowed(tool)
            && self.server_tools.allows_named(&aggregate_name, tool)
            && self.agent_tools.allows_named(&model_name, tool)
    }

    pub fn cli_visible(&self, backend: &str, deployment: &ToolFilter, tool: &Tool) -> bool {
        self.allows(backend, deployment, tool)
            && !self.defer_tools.is_model_visible_tool(&format!("{backend}__{}", tool.name))
    }
}

pub fn validate_server_alias(alias: &str) -> Result<(), RequestContextError> {
    if alias.trim().is_empty() || alias.contains("__") { Err(RequestContextError::InvalidAlias) } else { Ok(()) }
}
