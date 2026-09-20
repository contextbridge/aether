use crate::client::McpClient;
use rmcp::model::{ElicitRequestParams, ElicitResult, InputRequest, InputRequests, InputResponses, RequestMetaObject};

pub(crate) enum ElicitInputsError {
    UnsupportedInput,
    Serialize(serde_json::Error),
}

pub(crate) async fn elicit_inputs(
    client: &McpClient,
    requests: InputRequests,
) -> Result<(InputResponses, Vec<ElicitResult>), ElicitInputsError> {
    let mut responses = InputResponses::new();
    let mut results = Vec::new();
    for (key, request) in requests {
        let InputRequest::Elicitation(elicitation_request) = request else {
            return Err(ElicitInputsError::UnsupportedInput);
        };
        let extension_meta = elicitation_request.extensions.get::<RequestMetaObject>().cloned();
        let params = with_meta(elicitation_request.params, extension_meta);
        let result = client.dispatch_elicitation(params).await;
        let response = serde_json::to_value(&result).map_err(ElicitInputsError::Serialize)?;
        responses.insert(key, response);
        results.push(result);
    }
    Ok((responses, results))
}

pub(crate) fn with_meta(mut request: ElicitRequestParams, meta: Option<RequestMetaObject>) -> ElicitRequestParams {
    if let Some(meta) = meta {
        match &mut request {
            ElicitRequestParams::FormElicitationParams { meta: request_meta, .. }
            | ElicitRequestParams::UrlElicitationParams { meta: request_meta, .. } => {
                *request_meta = Some(meta);
            }
            _ => {}
        }
    }
    request
}
