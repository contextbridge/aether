use crate::client::McpClient;
use rmcp::model::{ElicitResult, InputRequest, InputRequests, InputResponses, RequestMetaObject, RequestParamsMeta};

/// Request-scoped destination for interactive tool input.
pub trait InputResponder: Send + Sync {
    fn elicit(&self, request: rmcp::model::ElicitRequestParams) -> futures::future::BoxFuture<'_, ElicitResult>;
}

impl InputResponder for McpClient {
    fn elicit(&self, request: rmcp::model::ElicitRequestParams) -> futures::future::BoxFuture<'_, ElicitResult> {
        Box::pin(self.dispatch_elicitation(request))
    }
}

pub(crate) enum ElicitInputsError {
    UnsupportedInput,
    Serialize(serde_json::Error),
}

pub(crate) async fn elicit_inputs(
    responder: &dyn InputResponder,
    requests: InputRequests,
) -> Result<(InputResponses, Vec<ElicitResult>), ElicitInputsError> {
    let mut responses = InputResponses::new();
    let mut results = Vec::new();
    for (key, request) in requests {
        let InputRequest::Elicitation(mut elicitation_request) = request else {
            return Err(ElicitInputsError::UnsupportedInput);
        };
        if let Some(meta) = elicitation_request.extensions.get::<RequestMetaObject>() {
            elicitation_request.params.meta_mut().get_or_insert_default().extend(meta.clone());
        }
        let result = responder.elicit(elicitation_request.params).await;
        let response = serde_json::to_value(&result).map_err(ElicitInputsError::Serialize)?;
        responses.insert(key, response);
        results.push(result);
    }
    Ok((responses, results))
}
