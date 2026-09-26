#![cfg(target_family = "wasm")]
#![doc = include_str!("../README.md")]

pub mod websocket;

mod conversation;
mod error;
mod event;

pub use error::ClientError;

use acp_utils::client::{AcpClientError, AcpClientHandle, connect_acp_client};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ClientCapabilities, ElicitationCapabilities, ElicitationFormCapabilities,
    ElicitationUrlCapabilities, Implementation, InitializeRequest, PromptRequest, ResumeSessionRequest, SessionId,
};
use conversation::ClientState;
use js_sys::{Function, Promise};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local};
use websocket::{WebSocketError, connect_websocket};

#[wasm_bindgen(typescript_custom_section)]
const TYPESCRIPT: &str = r#"
import type {
  ContentBlock,
  CreateElicitationRequest,
  CreateElicitationResponse,
  InitializeResponse,
  MessageId,
  NewSessionRequest,
  NewSessionResponse,
  PlanItems,
  PromptRequest,
  PromptResponse,
  ResumeSessionRequest,
  ResumeSessionResponse,
  SessionId,
  ToolCallUpdate,
  UpdateSessionNotification,
  UsageUpdate,
} from "@agentclientprotocol/sdk/experimental/v2";

/** Options for `AetherClient.connect`. */
export interface AetherClientOptions {
  /** WebSocket subprotocols to offer, as in `new WebSocket(url, protocols)`; a gateway can authenticate with them. */
  protocols?: string[];
}

/** The WebSocket close code and reason the browser reported, such as `1006` for a connection lost without a close frame. */
export interface WebSocketClose {
  code: number;
  reason: string;
}

/** A connection event, delivered in order to the `onEvent` callback given to `AetherClient.connect`. */
export type AetherClientEvent =
  | { type: "session_update"; notification: UpdateSessionNotification }
  | {
      type: "elicitation_request";
      request: CreateElicitationRequest;
      /** Answer the agent. Callable once; throws an `invalid_argument` error for a malformed response. */
      respond: (response: CreateElicitationResponse) => void;
    }
  | {
      type: "context_cleared" | "sub_agent_progress" | "auth_methods_updated" | "mcp_notification" | "git_diff_event";
      params: unknown;
    }
  | {
      type: "connection_closed";
      /** How the server or network closed the socket; `null` when this client closed it. */
      close: WebSocketClose | null;
    }
  | {
      /** Follows every event (or prompt call) that changes a session's conversation. */
      type: "conversation_changed";
      sessionId: SessionId;
      conversation: Conversation;
    };

/**
 * One session's conversation, reduced from its updates by the same rules as the `wisp` TUI.
 * Snapshots are immutable: an item that did not change is the same object in the next snapshot.
 */
export interface Conversation {
  items: ConversationItem[];
  turn: TurnPhase;
  activity: Activity;
  /** The agent's latest structured plan. */
  plan: PlanItems | null;
  contextUsage: UsageUpdate | null;
  /** The agent is compacting its context. */
  compacting: boolean;
}

/**
 * Where the current prompt is. A new prompt can only be sent while `idle`;
 * `completed_before_acceptance` means the turn ended before the agent answered the prompt request.
 */
export type TurnPhase = "idle" | "submitting" | "running" | "completed_before_acceptance";

/** What the agent is doing in the current turn; `thought` is the reasoning it is streaming while thinking. */
export interface Activity {
  phase: "idle" | "thinking" | "responding" | "requires_action" | "working";
  thought: string;
}

/** An item keeps its `id` for the life of the conversation; `revision` advances whenever it changes. */
export type ConversationItem = {
  id: number;
  messageId: MessageId | null;
  revision: number;
  /** A sealed item no longer changes. */
  state: "open" | "sealed";
} & (
  | {
      kind: "user" | "assistant";
      /** The message's content, with adjacent streamed text merged into one block. */
      content: ContentBlock[];
    }
  | { kind: "tool"; content: ToolCallState }
  | {
      /** Information from the client rather than the agent. */
      kind: "notice";
      content: string;
    }
);

export interface ToolCallState {
  status: ToolStatus;
  /** Sub-agents the tool spawned, with the tool calls each has made. */
  subAgents: SubAgent[];
  /** Every update the agent sent for this tool call, merged. */
  toolCall: ToolCallUpdate;
}

export type ToolStatus = "running" | "success" | { error: string };

export interface SubAgent {
  taskId: string;
  agentName: string;
  done: boolean;
  toolCalls: SubAgentToolCall[];
}

export interface SubAgentToolCall {
  id: string;
  name: string;
  rawInput: string;
  displayValue: string | null;
  status: ToolStatus;
}

export type AetherClientErrorCode = "connect_failed" | "protocol" | "invalid_argument" | "turn_in_progress";

/** The error every `AetherClient` method throws or rejects with. */
export interface AetherClientError extends Error {
  code: AetherClientErrorCode;
  /** Set on a `connect_failed` error when the socket closed before initialization completed. */
  close?: WebSocketClose;
}
"#;

/// An ACP v2 client connected to `aether server` over a browser WebSocket.
#[wasm_bindgen]
pub struct AetherClient {
    handle: AcpClientHandle,
    initialize_response: JsValue,
    state: Rc<ClientState>,
}

#[wasm_bindgen]
impl AetherClient {
    /// Connect to `url` and complete ACP initialization. Every connection event, starting now, goes to `onEvent`.
    pub async fn connect(
        url: String,
        #[wasm_bindgen(js_name = onEvent, unchecked_param_type = "(event: AetherClientEvent) => void")]
        on_event: Function,
        #[wasm_bindgen(unchecked_optional_param_type = "AetherClientOptions")] options: JsValue,
    ) -> Result<AetherClient, ClientError> {
        let options = from_js::<Option<ConnectOptions>>(options)?.unwrap_or_default();
        let (transport, close) = connect_websocket(&url, &options.protocols).await?;
        let client = connect_acp_client(transport, initialize_request()).await.map_err(|error| {
            close.get().map_or(ClientError::Client(error), |close| WebSocketError::Closed(close).into())
        })?;
        let initialize_response = to_js(&client.initialize_response)?;
        let state = Rc::new(ClientState::new(on_event));
        spawn_local(event::deliver(client.event_rx, Rc::clone(&state), close));
        Ok(Self { handle: client.handle, initialize_response, state })
    }

    /// The agent's initialize response: its info, capabilities, auth methods and `aether server` metadata.
    #[wasm_bindgen(getter, js_name = initializeResponse, unchecked_return_type = "InitializeResponse")]
    pub fn initialize_response(&self) -> JsValue {
        self.initialize_response.clone()
    }

    #[wasm_bindgen(js_name = newSession, unchecked_return_type = "Promise<NewSessionResponse>")]
    pub fn new_session(
        &self,
        #[wasm_bindgen(unchecked_param_type = "NewSessionRequest")] request: JsValue,
    ) -> Result<Promise, ClientError> {
        let response = self.handle.new_session(from_js(request)?);
        let state = Rc::clone(&self.state);
        Ok(respond(async move {
            let response = response.await?;
            state.track(response.session_id.clone());
            Ok(response)
        }))
    }

    /// Resume a session, starting its conversation over. With `replay`, its history arrives as
    /// `session_update` events, and rebuilds the conversation, before the promise resolves.
    #[wasm_bindgen(js_name = resumeSession, unchecked_return_type = "Promise<ResumeSessionResponse>")]
    pub fn resume_session(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ResumeSessionRequest")] request: JsValue,
        replay: bool,
    ) -> Result<Promise, ClientError> {
        let request: ResumeSessionRequest = from_js(request)?;
        self.state.restart(&request.session_id);
        Ok(if replay {
            respond(self.handle.resume_session_with_replay(request))
        } else {
            respond(self.handle.resume_session(request))
        })
    }

    /// Send a prompt, echoing it into the session's conversation. The promise resolves once the agent
    /// accepts it; the turn streams as `session_update` events. Throws `turn_in_progress` unless the
    /// session's turn is idle.
    #[wasm_bindgen(unchecked_return_type = "Promise<PromptResponse>")]
    pub fn prompt(
        &self,
        #[wasm_bindgen(unchecked_param_type = "PromptRequest")] request: JsValue,
    ) -> Result<Promise, ClientError> {
        let request: PromptRequest = from_js(request)?;
        let session_id = request.session_id.clone();
        self.state.start_prompt(&session_id, request.prompt.clone())?;
        let response = self.handle.prompt(request);
        let state = Rc::clone(&self.state);
        Ok(respond(async move {
            let response = response.await;
            state.settle_prompt(&session_id, response.as_ref());
            response
        }))
    }

    /// The session's conversation, or `undefined` for a session this client has not seen.
    #[wasm_bindgen(unchecked_return_type = "Conversation | undefined")]
    pub fn conversation(
        &self,
        #[wasm_bindgen(js_name = sessionId)] session_id: String,
    ) -> Result<JsValue, ClientError> {
        Ok(self.state.snapshot(&SessionId::new(session_id))?.unwrap_or(JsValue::UNDEFINED))
    }

    pub fn cancel(&self, #[wasm_bindgen(js_name = sessionId)] session_id: String) -> Result<(), ClientError> {
        Ok(self.handle.cancel(CancelSessionNotification::new(session_id))?)
    }

    /// Close the connection without cancelling or closing sessions. Resolves after `connection_closed` is emitted.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn disconnect(&self) -> Promise {
        let handle = self.handle.clone();
        future_to_promise(async move {
            handle.disconnect().await;
            Ok(JsValue::UNDEFINED)
        })
    }
}

pub(crate) fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, ClientError> {
    serde_wasm_bindgen::from_value(value).map_err(ClientError::InvalidArgument)
}

pub(crate) fn to_js<T: Serialize>(value: &T) -> Result<JsValue, ClientError> {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).map_err(ClientError::Conversion)
}

fn respond<T: Serialize>(response: impl Future<Output = Result<T, AcpClientError>> + 'static) -> Promise {
    future_to_promise(async move { Ok(to_js(&response.await.map_err(ClientError::from)?)?) })
}

#[derive(Default, Deserialize)]
struct ConnectOptions {
    #[serde(default)]
    protocols: Vec<String>,
}

fn initialize_request() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V2, Implementation::new("aether-acp-wasm", env!("CARGO_PKG_VERSION")))
        .capabilities(
            ClientCapabilities::new().elicitation(
                ElicitationCapabilities::new()
                    .form(ElicitationFormCapabilities::new())
                    .url(ElicitationUrlCapabilities::new()),
            ),
        )
}
