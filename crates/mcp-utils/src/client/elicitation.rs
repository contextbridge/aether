use crate::client::McpClient;
use rmcp::model::{ElicitResult, InputRequest, InputRequests, InputResponses};

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
        let result = client.dispatch_elicitation(elicitation_request.params).await;
        let response = serde_json::to_value(&result).map_err(ElicitInputsError::Serialize)?;
        responses.insert(key, response);
        results.push(result);
    }
    Ok((responses, results))
}
