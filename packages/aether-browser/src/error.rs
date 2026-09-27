use crate::js::to_js;
use crate::types::{AetherClientErrorCode, AetherClientErrorDetails, WebSocketClose};
use crate::websocket::InvalidRequest;
use acp_utils::client::AcpClientError;
use acp_utils::conversation::TurnInProgress;
use js_sys::Object;
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};

/// Errors surfaced to JavaScript as an `Error` carrying [`AetherClientErrorDetails`].
#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    InvalidRequest(#[from] InvalidRequest),
    #[error("WebSocket closed ({0})")]
    Closed(WebSocketClose),
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error("invalid argument: {0}")]
    InvalidArgument(#[source] serde_wasm_bindgen::Error),
    #[error("agent message is not representable in JavaScript: {0}")]
    Conversion(#[source] serde_wasm_bindgen::Error),
    #[error(transparent)]
    TurnInProgress(#[from] TurnInProgress),
    #[error("no session is open")]
    NoSession,
    #[error("the connection stopped")]
    Stopped,
    #[error("the elicitation was already answered")]
    AlreadyAnswered,
}

impl ClientError {
    pub fn details(&self) -> AetherClientErrorDetails {
        let code = match self {
            Self::InvalidRequest(_)
            | Self::Closed(_)
            | Self::Client(AcpClientError::AgentCrashed(_) | AcpClientError::InvalidAgentCommand(_)) => {
                AetherClientErrorCode::ConnectFailed
            }
            Self::Client(AcpClientError::Protocol(_)) | Self::Conversion(_) | Self::Stopped => {
                AetherClientErrorCode::Protocol
            }
            Self::InvalidArgument(_) => AetherClientErrorCode::InvalidArgument,
            Self::TurnInProgress(_) => AetherClientErrorCode::TurnInProgress,
            Self::NoSession => AetherClientErrorCode::NoSession,
            Self::AlreadyAnswered => AetherClientErrorCode::AlreadyAnswered,
        };
        let close = match self {
            Self::Closed(close) => Some(close.clone()),
            _ => None,
        };
        AetherClientErrorDetails { code, close }
    }
}

impl From<ClientError> for JsValue {
    fn from(error: ClientError) -> Self {
        let js_error = js_sys::Error::new(&error.to_string());
        if let Ok(details) = to_js(&error.details()) {
            Object::assign(&js_error, details.unchecked_ref());
        }
        js_error.into()
    }
}
