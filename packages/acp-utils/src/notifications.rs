//! Typed wire-format types for Aether's custom ACP extension requests and
//! notifications.
use std::path::PathBuf;

use agent_client_protocol::schema::AuthMethod;
use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};
pub use rmcp::model::CreateElicitationRequestParams;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use mcp_utils::status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub const AETHER_META_NAMESPACE: &str = "contextbridge/aether";

/// Parameters for `_aether/context_usage` notifications.
///
/// Per-turn fields (`input_tokens`, `output_tokens`, `cache_read_tokens`,
/// `cache_creation_tokens`, `reasoning_tokens`) come from the most recent
/// API response. The `total_*` fields are cumulative across the agent's
/// lifetime. The optional fields are `None` when the provider doesn't
/// expose that dimension; this is semantically distinct from `Some(0)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonRpcNotification)]
#[notification(method = "_aether/context_usage")]
pub struct ContextUsageParams {
    pub usage_ratio: Option<f64>,
    pub context_limit: Option<u32>,
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cache_read_tokens: u64,
    #[serde(default)]
    pub total_cache_creation_tokens: u64,
    #[serde(default)]
    pub total_reasoning_tokens: u64,
}

/// Parameters for `_aether/context_cleared` notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonRpcNotification)]
#[notification(method = "_aether/context_cleared")]
pub struct ContextClearedParams {}

/// Parameters for `_aether/auth_methods_updated` notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcNotification)]
#[notification(method = "_aether/auth_methods_updated")]
pub struct AuthMethodsUpdatedParams {
    pub auth_methods: Vec<AuthMethod>,
}

/// Request parameters for the `_aether/elicitation` ext method.
///
/// Carries the full RMCP elicitation request plus the originating server name
/// so the client can distinguish form vs URL mode and display which server is
/// requesting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonRpcRequest)]
#[request(method = "_aether/elicitation", response = ElicitationResponse)]
pub struct ElicitationParams {
    pub server_name: String,
    pub request: CreateElicitationRequestParams,
}

pub use rmcp::model::ElicitationAction;

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
    pub fn to_meta(&self) -> agent_client_protocol::schema::Meta {
        to_aether_meta(self)
    }

    #[must_use]
    pub fn from_meta(meta: Option<&agent_client_protocol::schema::Meta>) -> Self {
        from_aether_meta(meta)
    }
}

/// Parameters for `_aether/fork_options` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/fork_options", response = ForkOptionsResponse)]
#[serde(rename_all = "camelCase")]
pub struct ForkOptionsParams {
    pub session_id: String,
}

/// Response for `_aether/fork_options`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ForkOptionsResponse {
    pub session_id: String,
    pub options: Vec<WorkspaceOption>,
}

/// A workspace option displayed in the fork picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOption {
    pub name: String,
    pub path: PathBuf,
    pub subtitle: String,
}

/// Destination for `_aether/fork_session`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkspaceDestination {
    Existing { path: PathBuf },
    NewSibling { name: String },
}

/// Parameters for `_aether/fork_session` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_aether/fork_session", response = ForkSessionResponse)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionParams {
    pub session_id: String,
    pub destination: WorkspaceDestination,
}

/// Response for `_aether/fork_session`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionResponse {
    pub session_id: String,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub config_options: Vec<agent_client_protocol::schema::SessionConfigOption>,
}

/// Error returned by [`validate_workspace_name`].
#[derive(Debug, Clone)]
pub struct InvalidWorkspaceName(pub String);

impl std::fmt::Display for InvalidWorkspaceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid workspace name `{}`: must be non-empty with no path separators", self.0)
    }
}

impl std::error::Error for InvalidWorkspaceName {}

/// Validate a workspace name is non-empty and contains no path separators.
pub fn validate_workspace_name(name: &str) -> Result<(), InvalidWorkspaceName> {
    let valid = !name.trim().is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
    if valid { Ok(()) } else { Err(InvalidWorkspaceName(name.to_string())) }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AetherCapabilities {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prompt_search: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_preview: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace_fork: bool,
}

impl AetherCapabilities {
    #[must_use]
    pub fn prompt_search() -> Self {
        Self { prompt_search: true, session_preview: false, workspace_fork: false }
    }

    #[must_use]
    pub fn session_preview() -> Self {
        Self { prompt_search: false, session_preview: true, workspace_fork: false }
    }

    #[must_use]
    pub fn with_workspace_fork(mut self) -> Self {
        self.workspace_fork = true;
        self
    }

    #[must_use]
    pub fn to_meta(self) -> agent_client_protocol::schema::Meta {
        to_aether_meta(&self)
    }

    #[must_use]
    pub fn from_meta(meta: Option<&agent_client_protocol::schema::Meta>) -> Self {
        from_aether_meta(meta)
    }
}

fn to_aether_meta<T: Serialize>(value: &T) -> agent_client_protocol::schema::Meta {
    let mut meta = agent_client_protocol::schema::Meta::new();
    meta.insert(AETHER_META_NAMESPACE.to_string(), serde_json::json!(value));
    meta
}

fn from_aether_meta<T: DeserializeOwned + Default>(meta: Option<&agent_client_protocol::schema::Meta>) -> T {
    meta.and_then(|m| m.get(AETHER_META_NAMESPACE))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Response returned from the client for an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonRpcResponse)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    /// Structured form data when action is "accept".
    pub content: Option<serde_json::Value>,
}

pub use mcp_utils::client::UrlElicitationCompleteParams;

/// Server→client MCP extension notifications (relay → wisp).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcNotification)]
#[notification(method = "_aether/mcp_event")]
pub enum McpNotification {
    ServerStatus { servers: Vec<McpServerStatusEntry> },
    UrlElicitationComplete(UrlElicitationCompleteParams),
}

/// Client→server MCP extension requests (wisp → relay).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonRpcNotification)]
#[notification(method = "_aether/mcp_request")]
pub enum McpRequest {
    Authenticate { session_id: String, server_name: String },
}

/// Parameters for `_aether/sub_agent_progress` notifications.
///
/// This is the wire format sent from the ACP server (`aether-cli`) to clients like `wisp`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_aether/sub_agent_progress")]
pub struct SubAgentProgressParams {
    pub parent_tool_id: String,
    pub task_id: String,
    pub agent_name: String,
    pub event: SubAgentEvent,
}

/// Subset of agent message variants relevant for sub-agent status display.
///
/// The ACP server (`aether-cli`) converts `AgentMessage` to this type before
/// serializing, so the wire format only contains these known variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubAgentEvent {
    ToolCall { request: SubAgentToolRequest },
    ToolCallUpdate { update: SubAgentToolCallUpdate },
    ToolResult { result: SubAgentToolResult },
    ToolError { error: SubAgentToolError },
    Done,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentToolRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentToolCallUpdate {
    pub id: String,
    pub chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentToolResult {
    pub id: String,
    pub name: String,
    pub result_meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentToolError {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::JsonRpcMessage;
    use agent_client_protocol::schema::AuthMethodAgent;

    use super::*;

    #[test]
    fn wire_method_names_are_prefixed() {
        assert_eq!(ContextClearedParams::default().method(), "_aether/context_cleared");
        assert!(AuthMethodsUpdatedParams { auth_methods: vec![] }.method() == "_aether/auth_methods_updated");
        assert!(McpNotification::ServerStatus { servers: vec![] }.method() == "_aether/mcp_event");
        assert!(
            McpRequest::Authenticate { session_id: String::new(), server_name: String::new() }.method()
                == "_aether/mcp_request"
        );
        assert_eq!(PromptSearchParams { query: String::new(), limit: None }.method(), "_aether/prompt_search");
        assert_eq!(SessionPreviewParams { session_id: String::new() }.method(), "_aether/session_preview");
    }

    #[test]
    fn fork_options_wire_method_name() {
        let params = ForkOptionsParams { session_id: "s1".to_string() };
        assert_eq!(params.method(), "_aether/fork_options");
    }

    #[test]
    fn fork_session_wire_method_name() {
        let params = ForkSessionParams {
            session_id: "s1".to_string(),
            destination: WorkspaceDestination::NewSibling { name: "fork".to_string() },
        };
        assert_eq!(params.method(), "_aether/fork_session");
    }

    #[test]
    fn fork_options_roundtrip() {
        let params = ForkOptionsParams { session_id: "s1".to_string() };
        let untyped = params.to_untyped_message().expect("serializable");
        let parsed = ForkOptionsParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn fork_session_roundtrip() {
        let params = ForkSessionParams {
            session_id: "s1".to_string(),
            destination: WorkspaceDestination::NewSibling { name: "forked".to_string() },
        };
        let untyped = params.to_untyped_message().expect("serializable");
        let parsed = ForkSessionParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn workspace_destination_new_sibling_serializes_with_type_tag() {
        let dest = WorkspaceDestination::NewSibling { name: "big-refactor".to_string() };
        let json = serde_json::to_string(&dest).unwrap();
        assert!(json.contains("\"type\":\"newSibling\""));
        assert!(json.contains("\"name\":\"big-refactor\""));
        let parsed: WorkspaceDestination = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dest);
    }

    #[test]
    fn workspace_destination_existing_serializes_with_type_tag() {
        let dest = WorkspaceDestination::Existing { path: PathBuf::from("/home/dev/code/target") };
        let json = serde_json::to_string(&dest).unwrap();
        assert!(json.contains("\"type\":\"existing\""));
        assert!(json.contains("\"path\":\"/home/dev/code/target\""));
        let parsed: WorkspaceDestination = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dest);
    }

    #[test]
    fn fork_session_response_roundtrip() {
        let resp = ForkSessionResponse {
            session_id: "s1".to_string(),
            cwd: PathBuf::from("/home/dev/code/forked"),
            git_ref: Some("main".to_string()),
            config_options: Vec::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"cwd\":\"/home/dev/code/forked\""));
        assert!(json.contains("\"gitRef\":\"main\""));
        let parsed: ForkSessionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(parsed.cwd, PathBuf::from("/home/dev/code/forked"));
        assert_eq!(parsed.git_ref, Some("main".to_string()));
    }

    #[test]
    fn validate_workspace_name_rejects_invalid_names() {
        for name in ["", "   ", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(validate_workspace_name(name).is_err(), "expected rejection: {name:?}");
        }
    }

    #[test]
    fn validate_workspace_name_accepts_reasonable_names() {
        for name in ["fix-login", "aether2", "big_refactor", "feature.branch"] {
            assert!(validate_workspace_name(name).is_ok(), "expected acceptance: {name:?}");
        }
    }

    #[test]
    fn invalid_workspace_name_display() {
        let err = InvalidWorkspaceName("a/b".to_string());
        assert!(err.to_string().contains("invalid workspace name"));
        assert!(err.to_string().contains("a/b"));
    }

    #[test]
    fn prompt_search_capability_meta_roundtrip() {
        let meta = AetherCapabilities::prompt_search().to_meta();
        assert!(AetherCapabilities::from_meta(Some(&meta)).prompt_search);
        assert!(!AetherCapabilities::from_meta(None).prompt_search);
        assert!(!AetherCapabilities::from_meta(Some(&agent_client_protocol::schema::Meta::new())).prompt_search);
        let raw = serde_json::to_string(meta.get(AETHER_META_NAMESPACE).unwrap()).unwrap();
        assert!(raw.contains("promptSearch"));
        assert!(!raw.contains("sessionPreview"));
    }

    #[test]
    fn workspace_fork_capability_roundtrip() {
        let meta = AetherCapabilities::session_preview().with_workspace_fork().to_meta();
        let parsed = AetherCapabilities::from_meta(Some(&meta));
        assert!(parsed.session_preview);
        assert!(parsed.workspace_fork);
        assert!(!parsed.prompt_search);
        let raw = serde_json::to_string(meta.get(AETHER_META_NAMESPACE).unwrap()).unwrap();
        assert!(raw.contains("workspaceFork"));
    }

    #[test]
    fn workspace_fork_capability_defaults_to_false() {
        assert!(!AetherCapabilities::default().workspace_fork);
        assert!(!AetherCapabilities::prompt_search().workspace_fork);
    }

    #[test]
    fn context_usage_params_roundtrip() {
        let params = ContextUsageParams {
            usage_ratio: Some(0.75),
            context_limit: Some(100_000),
            input_tokens: 75_000,
            output_tokens: 1_200,
            cache_read_tokens: Some(40_000),
            cache_creation_tokens: Some(2_000),
            reasoning_tokens: Some(500),
            total_input_tokens: 200_000,
            total_output_tokens: 8_000,
            total_cache_read_tokens: 90_000,
            total_cache_creation_tokens: 5_000,
            total_reasoning_tokens: 1_500,
        };

        let untyped = params.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/context_usage");
        let parsed = ContextUsageParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn context_usage_params_omits_unset_optional_token_fields() {
        let params = ContextUsageParams {
            usage_ratio: Some(0.1),
            context_limit: Some(1_000),
            input_tokens: 100,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            total_cache_creation_tokens: 0,
            total_reasoning_tokens: 0,
        };

        let raw = serde_json::to_string(&params).unwrap();
        assert!(!raw.contains("\"cache_read_tokens\""));
        assert!(!raw.contains("\"cache_creation_tokens\""));
        assert!(!raw.contains("\"reasoning_tokens\""));
    }

    #[test]
    fn context_cleared_params_roundtrip() {
        let params = ContextClearedParams::default();
        let untyped = params.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/context_cleared");
        let parsed = ContextClearedParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn auth_methods_updated_roundtrip() {
        let params = AuthMethodsUpdatedParams {
            auth_methods: vec![
                AuthMethod::Agent(AuthMethodAgent::new("anthropic", "Anthropic").description("authenticated")),
                AuthMethod::Agent(AuthMethodAgent::new("openrouter", "OpenRouter")),
            ],
        };

        let untyped = params.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/auth_methods_updated");
        let parsed = AuthMethodsUpdatedParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn mcp_request_authenticate_roundtrip() {
        let msg = McpRequest::Authenticate {
            session_id: "session-0".to_string(),
            server_name: "my oauth server".to_string(),
        };

        let untyped = msg.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/mcp_request");
        let parsed = McpRequest::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, msg);
    }

    #[test]
    fn mcp_notification_server_status_roundtrip() {
        let msg = McpNotification::ServerStatus {
            servers: vec![
                McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
                McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
                    .with_auth_capability(McpServerAuthCapability::OAuth),
                McpServerStatusEntry::new("slack", McpServerStatus::Failed { error: "connection timeout".to_string() }),
            ],
        };

        let untyped = msg.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/mcp_event");
        let parsed = McpNotification::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, msg);
    }

    #[test]
    fn mcp_notification_url_elicitation_complete_roundtrip() {
        let msg = McpNotification::UrlElicitationComplete(UrlElicitationCompleteParams {
            server_name: "github".to_string(),
            elicitation_id: "el-456".to_string(),
        });

        let untyped = msg.to_untyped_message().expect("serializable");
        let parsed = McpNotification::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, msg);
    }

    #[test]
    fn sub_agent_progress_params_roundtrip() {
        let params = SubAgentProgressParams {
            parent_tool_id: "call_123".to_string(),
            task_id: "task_abc".to_string(),
            agent_name: "explorer".to_string(),
            event: SubAgentEvent::Done,
        };

        let untyped = params.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/sub_agent_progress");
    }

    #[test]
    fn elicitation_params_roundtrip() {
        use rmcp::model::{ElicitationSchema, EnumSchema};

        let params = ElicitationParams {
            server_name: "github".to_string(),
            request: CreateElicitationRequestParams::FormElicitationParams {
                meta: None,
                message: "Pick a color".to_string(),
                requested_schema: ElicitationSchema::builder()
                    .required_enum_schema(
                        "color",
                        EnumSchema::builder(vec!["red".into(), "green".into(), "blue".into()]).untitled().build(),
                    )
                    .build()
                    .unwrap(),
            },
        };

        let untyped = params.to_untyped_message().expect("serializable");
        assert_eq!(untyped.method(), "_aether/elicitation");
        let parsed = ElicitationParams::parse_message(untyped.method(), untyped.params()).expect("roundtrip");
        assert_eq!(parsed, params);
    }

    #[test]
    fn elicitation_params_url_variant_has_mode_field() {
        let params = ElicitationParams {
            server_name: "github".to_string(),
            request: CreateElicitationRequestParams::UrlElicitationParams {
                meta: None,
                message: "Authorize GitHub".to_string(),
                url: "https://github.com/login/oauth".to_string(),
                elicitation_id: "el-123".to_string(),
            },
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"mode\":\"url\""));
        assert!(json.contains("\"server_name\":\"github\""));
    }

    #[test]
    fn mcp_server_status_entry_serde_roundtrip() {
        let entry = McpServerStatusEntry::new("test-server", McpServerStatus::Connected { tool_count: 3 })
            .with_auth_capability(McpServerAuthCapability::OAuth);

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"auth_capability\":\"OAuth\""));
        assert!(json.contains("\"proxied\":false"));
        let parsed: McpServerStatusEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, entry);
        assert!(!parsed.proxied);
        assert!(parsed.can_authenticate());
    }

    #[test]
    fn mcp_server_status_entry_proxied_serde_roundtrip() {
        let entry = McpServerStatusEntry::new("math", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth)
            .with_proxied(true);

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"proxied\":true"));
        let parsed: McpServerStatusEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn deserialize_tool_call_event() {
        let json = r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{\"pattern\":\"test\"}"},"model_name":"m"}}"#;
        let event: SubAgentEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SubAgentEvent::ToolCall { .. }));
    }

    #[test]
    fn deserialize_tool_call_update_event() {
        let json = r#"{"ToolCallUpdate":{"update":{"id":"c1","chunk":"{\"pattern\":\"test\"}"},"model_name":"m"}}"#;
        let event: SubAgentEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SubAgentEvent::ToolCallUpdate { .. }));
    }

    #[test]
    fn deserialize_tool_result_event() {
        let json = r#"{"ToolResult":{"result":{"id":"c1","name":"grep","result_meta":{"display":{"title":"Grep","value":"'test' in src (3 matches)"}}}}}"#;
        let event: SubAgentEvent = serde_json::from_str(json).unwrap();
        match event {
            SubAgentEvent::ToolResult { result } => {
                let result_meta = result.result_meta.expect("expected result_meta");
                assert_eq!(result_meta.display.title, "Grep");
            }
            other => panic!("Expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_tool_error_event() {
        let json = r#"{"ToolError":{"error":{"id":"c1","name":"grep"}}}"#;
        let event: SubAgentEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SubAgentEvent::ToolError { .. }));
    }

    #[test]
    fn deserialize_done_event() {
        let event: SubAgentEvent = serde_json::from_str(r#""Done""#).unwrap();
        assert!(matches!(event, SubAgentEvent::Done));
    }

    #[test]
    fn deserialize_other_variant() {
        let event: SubAgentEvent = serde_json::from_str(r#""Other""#).unwrap();
        assert!(matches!(event, SubAgentEvent::Other));
    }

    #[test]
    fn tool_result_meta_map_roundtrip() {
        let meta: ToolResultMeta = ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into();
        let map = meta.clone().into_map();
        let parsed = ToolResultMeta::from_map(&map).expect("should deserialize ToolResultMeta");
        assert_eq!(parsed, meta);
    }
}
