//! Typed wire-format types for Aether's custom ACP extension requests and
//! notifications.
use agent_client_protocol::schema::AuthMethod;
use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};
pub use rmcp::model::CreateElicitationRequestParams;
use serde::{Deserialize, Serialize};

pub use mcp_utils::status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

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
