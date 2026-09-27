use acp_utils::conversation::{Activity, ConversationItem, TurnPhase};
use acp_utils::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, GitDiffEventPayload, McpNotification, SubAgentProgressParams,
};
use agent_client_protocol::schema::v2::{PlanItems, SessionId, UpdateSessionNotification, UsageUpdate};
use js_sys::Array;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::fmt;
use wasm_bindgen::JsValue;

#[derive(Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AetherClientEvent<'a> {
    SessionUpdate {
        notification: &'a UpdateSessionNotification,
    },
    ElicitationRequest {
        #[serde(with = "serde_wasm_bindgen::preserve")]
        #[schemars(schema_with = "elicitation_schema")]
        elicitation: JsValue,
    },
    ContextCleared {
        params: &'a ContextClearedParams,
    },
    SubAgentProgress {
        params: &'a SubAgentProgressParams,
    },
    AuthMethodsUpdated {
        params: &'a AuthMethodsUpdatedParams,
    },
    McpNotification {
        params: &'a McpNotification,
    },
    GitDiffEvent {
        #[schemars(with = "serde_json::Value")]
        params: &'a GitDiffEventPayload,
    },
    ConnectionClosed {
        close: Option<WebSocketClose>,
    },
    ConversationChanged {
        conversation: Option<ConversationSnapshot<'a>>,
    },
}

/// The current session's conversation
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "Conversation")]
pub struct ConversationSnapshot<'a> {
    pub session_id: &'a SessionId,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    #[schemars(with = "Vec<ConversationItem>")]
    pub items: Array,
    pub turn: TurnPhase,
    pub activity: Activity,
    pub plan: Option<&'a PlanItems>,
    pub context_usage: Option<&'a UsageUpdate>,
    pub compacting: bool,
}

/// Options for `AetherClient.connect`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AetherClientOptions {
    /// WebSocket subprotocols.
    #[serde(default)]
    pub protocols: Vec<String>,
}

/// `AetherClient` error payload.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AetherClientErrorDetails {
    pub code: AetherClientErrorCode,
    /// How the socket closed, when it closed before initialization completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<WebSocketClose>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherClientErrorCode {
    ConnectFailed,
    Protocol,
    InvalidArgument,
    TurnInProgress,
    NoSession,
    AlreadyAnswered,
}

/// close frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct WebSocketClose {
    pub code: u16,
    pub reason: String,
}

impl fmt::Display for WebSocketClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "code {}", self.code)?;
        if !self.reason.is_empty() {
            write!(f, ": {}", self.reason)?;
        }
        Ok(())
    }
}

fn elicitation_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "tsType": "Elicitation" })
}
