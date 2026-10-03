use crate::ClientError;
use crate::connection::{Command, Connection, Reply};
use crate::js::{from_js, to_js};
use crate::types::AetherClientOptions;
use crate::websocket::connect_websocket;
use acp_utils::client::{connect_acp_client, initialize_request};
use acp_utils::notifications::RemoteServerInfo;
use agent_client_protocol::schema::v2::Implementation;
use js_sys::{Function, Promise};
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use wasm_bindgen::prelude::*;

/// An ACP v2 client connected to `aether server` over a browser WebSocket.
#[wasm_bindgen]
pub struct AetherClient {
    commands: mpsc::UnboundedSender<Command>,
    initialize_response: JsValue,
    remote: JsValue,
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
        let options = from_js::<Option<AetherClientOptions>>(options)?.unwrap_or_default();
        let (transport, close) = connect_websocket(&url, &options.protocols)?;
        let info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        let client = connect_acp_client(transport, initialize_request(info))
            .await
            .map_err(|error| close.get().map_or(ClientError::Client(error), ClientError::Closed))?;

        let initialize_response = to_js(&client.initialize_response)?;
        let remote = to_js(&RemoteServerInfo::from_meta(client.initialize_response.meta.as_ref()))?;
        let commands = Connection::spawn(client.handle, client.event_rx, on_event, close);

        Ok(Self { commands, initialize_response, remote })
    }

    /// The agent's initialize response: its info, capabilities, auth methods and `aether server` metadata.
    #[wasm_bindgen(getter, js_name = initializeResponse, unchecked_return_type = "InitializeResponse")]
    pub fn initialize_response(&self) -> JsValue {
        self.initialize_response.clone()
    }

    #[wasm_bindgen(getter, unchecked_return_type = "RemoteServerInfo | null")]
    pub fn remote(&self) -> JsValue {
        self.remote.clone()
    }

    /// Create a session and make it the current one, emitting its (empty) conversation.
    #[wasm_bindgen(js_name = newSession, unchecked_return_type = "Promise<NewSessionResponse>")]
    pub fn new_session(&self, #[wasm_bindgen(unchecked_param_type = "NewSessionRequest")] request: JsValue) -> Promise {
        self.request(request, Command::NewSession)
    }

    /// Resume a session and make it the current one, with its conversation starting over. With `replayFrom` set, its
    /// history arrives as `session_update` events, and rebuilds the conversation, before the promise resolves.
    #[wasm_bindgen(js_name = resumeSession, unchecked_return_type = "Promise<ResumeSessionResponse>")]
    pub fn resume_session(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ResumeSessionRequest")] request: JsValue,
    ) -> Promise {
        self.request(request, Command::ResumeSession)
    }

    #[wasm_bindgen(unchecked_return_type = "Promise<PromptResponse>")]
    pub fn prompt(&self, #[wasm_bindgen(unchecked_param_type = "ContentBlock[]")] prompt: JsValue) -> Promise {
        self.request(prompt, Command::Prompt)
    }

    /// Cancel the current session's turn.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn cancel(&self) -> Promise {
        self.send(Command::Cancel)
    }

    /// Close the current session, and stop tracking it once the agent has closed it.
    #[wasm_bindgen(js_name = closeSession, unchecked_return_type = "Promise<CloseSessionResponse>")]
    pub fn close_session(&self) -> Promise {
        self.send(Command::CloseSession)
    }

    /// Close the connection without cancelling or closing the session. Resolves after `connection_closed` is emitted.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn disconnect(&self) -> Promise {
        self.send(Command::Disconnect)
    }

    fn request<T: DeserializeOwned>(&self, value: JsValue, command: fn(T, Reply) -> Command) -> Promise {
        match from_js(value) {
            Ok(value) => self.send(|reply| command(value, reply)),
            Err(error) => Promise::reject(&error.into()),
        }
    }

    fn send(&self, command: impl FnOnce(Reply) -> Command) -> Promise {
        let (reply, promise) = Reply::promise();
        let _ = self.commands.send(command(reply));
        promise
    }
}
