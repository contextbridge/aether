use crate::to_js;
use crate::websocket::{WebSocketClose, WebSocketError};
use acp_utils::client::AcpClientError;
use js_sys::Reflect;
use thiserror::Error;
use wasm_bindgen::JsValue;

/// Errors surfaced to JavaScript as an `Error` carrying a stable `code`.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    WebSocket(#[from] WebSocketError),
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error("invalid argument: {0}")]
    InvalidArgument(#[source] serde_wasm_bindgen::Error),
    #[error("agent message is not representable in JavaScript: {0}")]
    Conversion(#[source] serde_wasm_bindgen::Error),
    #[error("the session is already in a turn")]
    TurnInProgress,
}

impl ClientError {
    /// The `code` property of the JavaScript error: `connect_failed`, `protocol`, `invalid_argument` or
    /// `turn_in_progress`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::WebSocket(_)
            | Self::Client(AcpClientError::AgentCrashed(_) | AcpClientError::InvalidAgentCommand(_)) => {
                "connect_failed"
            }
            Self::Client(AcpClientError::Protocol(_)) | Self::Conversion(_) => "protocol",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::TurnInProgress => "turn_in_progress",
        }
    }

    /// The `close` property of the JavaScript error: how the socket closed, when it closed while connecting.
    pub fn close(&self) -> Option<&WebSocketClose> {
        match self {
            Self::WebSocket(WebSocketError::Closed(close)) => Some(close),
            _ => None,
        }
    }
}

impl From<ClientError> for JsValue {
    fn from(error: ClientError) -> Self {
        let js_error = js_sys::Error::new(&error.to_string());
        let _ = Reflect::set(&js_error, &"code".into(), &error.code().into());
        if let Some(close) = error.close().and_then(|close| to_js(close).ok()) {
            let _ = Reflect::set(&js_error, &"close".into(), &close);
        }
        js_error.into()
    }
}
