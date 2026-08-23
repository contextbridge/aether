use rmcp::RoleServer;
use rmcp::model::{
    ClientCapabilities, ElicitRequestParams, ErrorData, InputRequest, InputRequests, InputRequiredResult,
    InputResponses,
};
use rmcp::service::RequestContext;
use serde::de::DeserializeOwned;

pub const ELICITATION_UNSUPPORTED: &str = "This tool needs to ask the user for input, but the connected client does not support \
     interactive input (MCP elicitation over protocol 2026-07-28 or newer).";

/// The next action in a server-side MRTR round.
#[derive(Debug)]
pub enum MrtrAction {
    /// Ask the client for a batch of inputs and end the current tool call.
    Request(InputRequiredResult),
    /// Resume the tool call with the complete response batch.
    Resume(InputResponses),
    /// End the tool call because the client cannot fulfill an input request.
    Abort(AbortReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    UnsupportedInput { key: String, kind: InputKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    FormElicitation,
    UrlElicitation,
    Sampling,
    Roots,
    Unknown,
}

impl MrtrAction {
    /// Validate an outgoing request batch against the negotiated client.
    /// Responses bypass validation because the prior batch was already fulfilled.
    pub fn validate_for_client(self, context: &RequestContext<RoleServer>) -> Self {
        let Self::Request(result) = &self else {
            return self;
        };
        let Some(requests) = result.input_requests.as_ref() else {
            return self;
        };
        let capabilities = context.client_capabilities();
        match validate_input_requests(capabilities.as_ref(), requests) {
            Ok(()) => self,
            Err(reason) => Self::Abort(reason),
        }
    }
}

impl AbortReason {
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedInput { key, kind } => {
                format!("Client does not support {kind} required by input request '{key}'")
            }
        }
    }
}

impl std::fmt::Display for InputKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::FormElicitation => "form elicitation",
            Self::UrlElicitation => "URL elicitation",
            Self::Sampling => "sampling",
            Self::Roots => "roots",
            Self::Unknown => "this input kind",
        };
        f.write_str(name)
    }
}

pub fn validate_input_requests(
    capabilities: Option<&ClientCapabilities>,
    requests: &InputRequests,
) -> Result<(), AbortReason> {
    for (key, request) in requests {
        let kind = input_kind(request);
        let supported = capabilities.is_some_and(|capabilities| supports_input(capabilities, kind));
        if !supported {
            return Err(AbortReason::UnsupportedInput { key: key.clone(), kind });
        }
    }
    Ok(())
}

/// Deserialize one keyed response from a complete MRTR response batch.
pub fn parse_response<T: DeserializeOwned>(responses: &InputResponses, key: &str) -> Result<T, ErrorData> {
    let response = responses
        .get(key)
        .ok_or_else(|| ErrorData::invalid_params(format!("missing input response for '{key}'"), None))?;
    serde_json::from_value(response.clone())
        .map_err(|e| ErrorData::invalid_params(format!("invalid input response for '{key}': {e}"), None))
}

fn supports_input(capabilities: &ClientCapabilities, kind: InputKind) -> bool {
    match kind {
        InputKind::FormElicitation => capabilities
            .elicitation
            .as_ref()
            .is_some_and(|elicitation| elicitation.form.is_some() || elicitation.url.is_none()),
        InputKind::UrlElicitation => {
            capabilities.elicitation.as_ref().is_some_and(|elicitation| elicitation.url.is_some())
        }
        InputKind::Sampling => capabilities.sampling.is_some(),
        InputKind::Roots => capabilities.roots.is_some(),
        InputKind::Unknown => false,
    }
}

fn input_kind(request: &InputRequest) -> InputKind {
    #[allow(unreachable_patterns)]
    match request {
        InputRequest::CreateMessage(_) => InputKind::Sampling,
        InputRequest::Elicitation(request) => elicitation_kind(&request.params),
        InputRequest::ListRoots(_) => InputKind::Roots,
        _ => InputKind::Unknown,
    }
}

fn elicitation_kind(params: &ElicitRequestParams) -> InputKind {
    #[allow(unreachable_patterns)]
    match params {
        ElicitRequestParams::FormElicitationParams { .. } => InputKind::FormElicitation,
        ElicitRequestParams::UrlElicitationParams { .. } => InputKind::UrlElicitation,
        _ => InputKind::Unknown,
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

        let error = validate_input_requests(Some(&capabilities), &requests).unwrap_err();

        assert_eq!(error, AbortReason::UnsupportedInput { key: "url".to_string(), kind: InputKind::UrlElicitation });
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

        validate_input_requests(Some(&capabilities), &requests).unwrap();
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
