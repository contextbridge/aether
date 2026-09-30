//! Typed wire-format types for Aether's custom ACP extension requests and
//! notifications.
use std::path::PathBuf;

use agent_client_protocol::schema::v2::{AuthMethod, Meta, SessionId, ToolCallUpdate};
use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use clankerdiff_protocol::client::ClientCommand;
use clankerdiff_protocol::shared::{DocumentUpdate, Event};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use utils::display_meta::{ToolDisplayMeta, ToolResultMeta};
pub use utils::mcp_status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

use crate::meta::{from_meta, to_meta};

pub const AETHER_META_NAMESPACE: &str = "contextbridge/aether";

/// Remote host discovery, advertised on the initialize response only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteServerInfo {
    pub cwd: PathBuf,
    pub session_id: Option<SessionId>,
}

impl RemoteServerInfo {
    #[must_use]
    pub fn to_meta(&self) -> Meta {
        to_meta(&RemoteInitializationMeta { remote: Some(self.clone()) }, Some(AETHER_META_NAMESPACE))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn from_meta(meta: Option<&Meta>) -> Option<Self> {
        from_meta::<RemoteInitializationMeta>(meta, Some(AETHER_META_NAMESPACE)).remote
    }
}

/// Parameters for `_aether/session_usage` notifications.
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonRpcNotification)]
#[notification(method = "_aether/session_usage")]
pub struct SessionUsageParams {
    pub usage: llm::SessionUsageEvent,
}

/// Parameters for `_aether/context_cleared` notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, JsonRpcNotification)]
#[notification(method = "_aether/context_cleared")]
#[serde(rename_all = "camelCase")]
pub struct ContextClearedParams {
    pub session_id: SessionId,
}

/// Parameters for `_aether/auth_methods_updated` notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, JsonRpcNotification)]
#[notification(method = "_aether/auth_methods_updated")]
#[serde(rename_all = "camelCase")]
pub struct AuthMethodsUpdatedParams {
    pub auth_methods: Vec<AuthMethod>,
}

/// Parameters for the `_aether/prompt_search` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/prompt_search", response = PromptSearchResponse)]
#[serde(rename_all = "camelCase")]
pub struct PromptSearchParams {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Response for the `_aether/prompt_search` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct PromptSearchResponse {
    pub query: String,
    pub results: Vec<PromptSearchResult>,
    pub truncated: bool,
}

/// A single prompt-history search hit.
///
/// `match_start` and `match_end` are UTF-8 byte offsets into `prompt` and are
/// guaranteed to fall on char boundaries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptSearchResult {
    pub session_id: String,
    pub cwd: PathBuf,
    pub session_created_at: String,
    pub prompt: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/session_preview", response = SessionPreviewResponse)]
#[serde(rename_all = "camelCase")]
pub struct SessionPreviewParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct SessionPreviewResponse {
    pub session_id: String,
    pub cwd: PathBuf,
    pub created_at: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_mode: Option<String>,
    pub transcript: Vec<SessionPreviewTurn>,
    pub tool_call_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionPreviewTurn {
    pub role: SessionPreviewRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionPreviewRole {
    User,
    Assistant,
}

/// Parameters for the `_aether/workspace_list` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/workspace_list", response = WorkspaceListResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListParams {
    pub session_id: String,
}

/// Response for the `_aether/workspace_list` request: every managed workspace
/// originating from the same git repository as the session's working directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResponse {
    pub workspaces: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub is_current: bool,
}

/// Parameters for the `_aether/workspace_move` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/workspace_move", response = WorkspaceMoveResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMoveParams {
    pub session_id: String,
    pub target: WorkspaceMoveTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkspaceMoveTarget {
    Existing { path: PathBuf },
    New { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMoveResponse {
    pub new_cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionDisplayMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_mode: Option<String>,
}

impl SessionDisplayMeta {
    #[must_use]
    pub fn new(model: impl Into<String>, selected_mode: Option<String>) -> Self {
        Self { model: Some(model.into()), selected_mode }
    }

    #[must_use]
    pub fn to_meta(&self) -> Meta {
        to_meta(self, Some(AETHER_META_NAMESPACE)).unwrap_or_default()
    }

    #[must_use]
    pub fn from_meta(meta: Option<&Meta>) -> Self {
        from_meta(meta, Some(AETHER_META_NAMESPACE))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AetherCapabilities {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prompt_search: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_preview: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace_move: bool,
}

impl AetherCapabilities {
    #[must_use]
    pub fn to_meta(self) -> Meta {
        to_meta(&self, Some(AETHER_META_NAMESPACE)).unwrap_or_default()
    }

    #[must_use]
    pub fn from_meta(meta: Option<&Meta>) -> Self {
        from_meta(meta, Some(AETHER_META_NAMESPACE))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_aether/git_diff")]
#[serde(rename_all = "camelCase")]
pub struct GitDiffCommandPayload {
    pub session_id: String,
    #[serde(flatten)]
    pub command: ClientCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_aether/git_diff_event")]
#[serde(rename_all = "camelCase")]
pub struct GitDiffEventPayload {
    pub session_id: SessionId,
    #[serde(flatten)]
    pub event: Event<DocumentUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_aether/git_diff_close")]
#[serde(rename_all = "camelCase")]
pub struct GitDiffClosePayload {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/workspace_status", response = WorkspaceStatusResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatusPayload {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatusResponse {
    pub display_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
}

/// Server→client MCP extension notifications (relay → wisp).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, JsonRpcNotification)]
#[notification(method = "_aether/mcp_event")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpNotification {
    ServerStatus { servers: Vec<McpServerStatusEntry> },
}

/// Client→server MCP extension requests (wisp → relay).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcNotification)]
#[notification(method = "_aether/mcp_request")]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum McpRequest {
    Authenticate { session_id: String, server_name: String },
}

/// Parameters for `_aether/sub_agent_progress` notifications.
///
/// This is the wire format sent from the ACP server (`aether-cli`) to clients like `wisp`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, JsonRpcNotification)]
#[notification(method = "_aether/sub_agent_progress")]
#[serde(rename_all = "camelCase")]
pub struct SubAgentProgressParams {
    pub session_id: SessionId,
    pub parent_tool_id: String,
    pub task_id: String,
    pub agent_name: String,
    pub event: SubAgentEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubAgentEvent {
    Started,
    ToolCallUpdate(Box<ToolCallUpdate>),
    Done,
}

#[derive(Default, Serialize, Deserialize)]
struct RemoteInitializationMeta {
    remote: Option<RemoteServerInfo>,
}
