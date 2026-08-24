use rmcp::model::{ClientCapabilities, ElicitRequestParams, ErrorData, InputRequest, InputRequests, InputResponses};
use serde::de::DeserializeOwned;

pub const ELICITATION_UNSUPPORTED: &str = "This tool needs to ask the user for input, but the connected client does not support \
     interactive input (MCP elicitation over protocol 2026-07-28 or newer).";

pub fn input_requests_supported(capabilities: Option<&ClientCapabilities>, requests: &InputRequests) -> bool {
    requests.values().all(|request| capabilities.is_some_and(|capabilities| supports_input(capabilities, request)))
}

/// Deserialize one keyed response from a complete MRTR response batch.
pub fn parse_response<T: DeserializeOwned>(responses: &InputResponses, key: &str) -> Result<T, ErrorData> {
    let response = responses
        .get(key)
        .ok_or_else(|| ErrorData::invalid_params(format!("missing input response for '{key}'"), None))?;
    serde_json::from_value(response.clone())
        .map_err(|e| ErrorData::invalid_params(format!("invalid input response for '{key}': {e}"), None))
}

fn supports_input(capabilities: &ClientCapabilities, request: &InputRequest) -> bool {
    #[allow(unreachable_patterns)]
    match request {
        InputRequest::CreateMessage(_) => capabilities.sampling.is_some(),
        InputRequest::Elicitation(request) => supports_elicitation(capabilities, &request.params),
        InputRequest::ListRoots(_) => capabilities.roots.is_some(),
        _ => false,
    }
}

fn supports_elicitation(capabilities: &ClientCapabilities, params: &ElicitRequestParams) -> bool {
    #[allow(unreachable_patterns)]
    match params {
        ElicitRequestParams::FormElicitationParams { .. } => capabilities
            .elicitation
            .as_ref()
            .is_some_and(|elicitation| elicitation.form.is_some() || elicitation.url.is_none()),
        ElicitRequestParams::UrlElicitationParams { .. } => {
            capabilities.elicitation.as_ref().is_some_and(|elicitation| elicitation.url.is_some())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        ClientCapabilities, ElicitRequest, ElicitRequestParams, ElicitationCapability, ElicitationSchema,
        FormElicitationCapability, InputRequest, UrlElicitationCapability,
    };
    use serde_json::json;

    #[test]
    fn validates_capabilities_against_every_request_in_the_batch() {
        let requests = InputRequests::from([
            ("form".to_string(), elicitation("Form")),
            ("url".to_string(), url_elicitation("URL")),
        ]);
        let mut capabilities = ClientCapabilities::default();
        capabilities.elicitation = Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));

        assert!(!input_requests_supported(Some(&capabilities), &requests));
    }

    #[test]
    fn validates_a_mixed_elicitation_batch_when_all_capabilities_are_present() {
        let requests = InputRequests::from([
            ("form".to_string(), elicitation("Form")),
            ("url".to_string(), url_elicitation("URL")),
        ]);
        let mut capabilities = ClientCapabilities::default();
        capabilities.elicitation = Some(
            ElicitationCapability::new()
                .with_form(FormElicitationCapability::new())
                .with_url(UrlElicitationCapability::new()),
        );

        assert!(input_requests_supported(Some(&capabilities), &requests));
    }

    #[test]
    fn parse_response_deserializes_a_key_from_the_batch() {
        let responses = InputResponses::from([("answer".to_string(), json!(42))]);

        assert_eq!(parse_response::<u64>(&responses, "answer").unwrap(), 42);
    }

    #[test]
    fn parse_response_rejects_a_missing_key() {
        let error = parse_response::<u64>(&InputResponses::new(), "answer").unwrap_err();

        assert_eq!(error.message, "missing input response for 'answer'");
    }

    fn elicitation(message: &str) -> InputRequest {
        InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: message.to_string(),
            requested_schema: ElicitationSchema::builder().build().unwrap(),
        }))
    }

    fn url_elicitation(message: &str) -> InputRequest {
        InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: message.to_string(),
            url: "https://example.com/input".to_string(),
            elicitation_id: "id".to_string(),
        }))
    }
}
