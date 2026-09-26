use crate::conversation::ClientState;
use crate::websocket::{CloseStatus, WebSocketClose};
use crate::{ClientError, from_js, to_js};
use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, GitDiffEventPayload, McpNotification, SubAgentProgressParams,
};
use agent_client_protocol::schema::v2::{
    CreateElicitationRequest, CreateElicitationResponse, UpdateSessionNotification,
};
use agent_client_protocol::{self as acp, Responder};
use js_sys::Reflect;
use serde::Serialize;
use std::rc::Rc;
use tokio::sync::mpsc;
use wasm_bindgen::prelude::*;

/// Deliver each connection event to JS, in order, until the connection closes. An
/// event that changes a conversation is followed by that conversation's snapshot.
pub(crate) async fn deliver(mut events: mpsc::UnboundedReceiver<AcpEvent>, state: Rc<ClientState>, close: CloseStatus) {
    while let Some(event) = events.recv().await {
        let changed = state.reduce(&event);
        if let Ok(event) = to_js_event(event, &close) {
            state.emit(&event);
        }
        for session_id in &changed {
            state.emit_changed(session_id);
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsEvent<'a> {
    SessionUpdate { notification: &'a UpdateSessionNotification },
    ElicitationRequest { request: &'a CreateElicitationRequest },
    ContextCleared { params: &'a ContextClearedParams },
    SubAgentProgress { params: &'a SubAgentProgressParams },
    AuthMethodsUpdated { params: &'a AuthMethodsUpdatedParams },
    McpNotification { params: &'a McpNotification },
    GitDiffEvent { params: &'a GitDiffEventPayload },
    ConnectionClosed { close: Option<WebSocketClose> },
}

fn to_js_event(event: AcpEvent, close: &CloseStatus) -> Result<JsValue, ClientError> {
    match event {
        AcpEvent::SessionUpdate(notification) => to_js(&JsEvent::SessionUpdate { notification: &notification }),
        AcpEvent::ElicitationRequest { params, responder } => {
            let event = match to_js(&JsEvent::ElicitationRequest { request: &params }) {
                Ok(event) => event,
                Err(error) => {
                    let _ = responder.respond_with_error(acp::Error::internal_error());
                    return Err(error);
                }
            };
            let _ = Reflect::set(&event, &"respond".into(), &respond_once(responder));
            Ok(event)
        }
        AcpEvent::ContextCleared(params) => to_js(&JsEvent::ContextCleared { params: &params }),
        AcpEvent::SubAgentProgress(params) => to_js(&JsEvent::SubAgentProgress { params: &params }),
        AcpEvent::AuthMethodsUpdated(params) => to_js(&JsEvent::AuthMethodsUpdated { params: &params }),
        AcpEvent::McpNotification(params) => to_js(&JsEvent::McpNotification { params: &params }),
        AcpEvent::GitDiffEvent(params) => to_js(&JsEvent::GitDiffEvent { params: &params }),
        AcpEvent::ConnectionClosed => to_js(&JsEvent::ConnectionClosed { close: close.get() }),
    }
}

fn respond_once(responder: Responder<CreateElicitationResponse>) -> JsValue {
    Closure::<dyn FnMut(JsValue) -> Result<(), JsValue>>::once_into_js(move |response: JsValue| {
        let response = match from_js::<CreateElicitationResponse>(response) {
            Ok(response) => response,
            Err(error) => {
                let _ = responder.respond_with_error(acp::Error::invalid_params());
                return Err(error.into());
            }
        };
        responder.respond(response).map_err(|error| ClientError::from(AcpClientError::Protocol(error)).into())
    })
}
