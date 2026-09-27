use crate::ClientError;
use crate::js::{from_js, to_js};
use acp_utils::client::AcpClientError;
use agent_client_protocol::schema::v2::{CreateElicitationRequest, CreateElicitationResponse, ElicitationAction};
use agent_client_protocol::{self as acp, Responder};
use wasm_bindgen::prelude::*;

/// A form or URL elicitation from the agent.
#[wasm_bindgen]
pub struct Elicitation {
    request: JsValue,
    responder: Option<Responder<CreateElicitationResponse>>,
}

#[wasm_bindgen]
impl Elicitation {
    #[wasm_bindgen(getter, unchecked_return_type = "CreateElicitationRequest")]
    pub fn request(&self) -> JsValue {
        self.request.clone()
    }

    pub fn respond(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "CreateElicitationResponse")] response: JsValue,
    ) -> Result<(), ClientError> {
        let response: CreateElicitationResponse = from_js(response)?;
        let responder = self.responder.take().ok_or(ClientError::AlreadyAnswered)?;
        Ok(responder.respond(response).map_err(AcpClientError::Protocol)?)
    }

    #[wasm_bindgen(js_name = toJSON, unchecked_return_type = "CreateElicitationRequest")]
    pub fn to_json(&self) -> JsValue {
        self.request.clone()
    }

    pub(crate) fn new(
        request: &CreateElicitationRequest,
        responder: Responder<CreateElicitationResponse>,
    ) -> Result<Self, ClientError> {
        match to_js(request) {
            Ok(request) => Ok(Self { request, responder: Some(responder) }),
            Err(error) => {
                let _ = responder.respond_with_error(acp::Error::internal_error());
                Err(error)
            }
        }
    }
}

impl Drop for Elicitation {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(CreateElicitationResponse::new(ElicitationAction::Cancel));
        }
    }
}
