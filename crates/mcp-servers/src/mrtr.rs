use rmcp::{
    ErrorData, RoleServer,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext},
    model::{
        CallToolRequestParams, CallToolResponse, ClientCapabilities, ElicitRequest, ElicitRequestParams, ElicitResult,
        InputRequest, InputRequests, InputRequiredResult, ProtocolVersion, RequestStateCodec, SealOptions,
    },
    service::RequestContext,
};
use serde_json::{Map, Value};
use std::time::Duration;

const STATE_TTL: Duration = Duration::from_mins(30);
const STATE_PAYLOAD: &[u8] = b"aether-mrtr-v1";

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct MrtrToolRequest {
    pub tool_name: String,
    pub original_arguments: Option<Map<String, Value>>,
    pub input_responses: Option<rmcp::model::InputResponses>,
    pub request_state: Option<String>,
    pub protocol_version: ProtocolVersion,
    pub client_capabilities: ClientCapabilities,
}

pub(crate) fn route_tool_call<'a, S: Send + Sync + 'static>(
    service: &'a S,
    router: &'a ToolRouter<S>,
    mut request: CallToolRequestParams,
    mut context: RequestContext<RoleServer>,
) -> impl std::future::Future<Output = Result<CallToolResponse, ErrorData>> + 'a {
    let original_arguments = request.arguments.clone();
    let input_responses = request.input_responses.take();
    let request_state = request.request_state.take();
    let protocol_version = context.protocol_version().unwrap_or(ProtocolVersion::V_2025_11_25);
    let client_capabilities = context.client_capabilities().unwrap_or_default();
    context.extensions.insert(MrtrToolRequest {
        tool_name: request.name.to_string(),
        original_arguments,
        input_responses,
        request_state,
        protocol_version,
        client_capabilities,
    });
    async move { router.call(ToolCallContext::new(service, request, context)).await }
}

pub(crate) fn require_form_elicitation(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    let version = context.protocol_version().ok_or_else(|| {
        ErrorData::invalid_request("interactive tools require a negotiated MCP protocol version", None)
    })?;
    if version < ProtocolVersion::V_2026_07_28 {
        return Err(ErrorData::invalid_request(
            "interactive tools require MCP protocol 2026-07-28 MRTR; this client negotiated an older protocol",
            None,
        ));
    }
    let capabilities = context.client_capabilities().ok_or_else(|| {
        ErrorData::invalid_request("interactive tools require the client's elicitation capability", None)
    })?;
    if capabilities.elicitation.as_ref().and_then(|elicitation| elicitation.form.as_ref()).is_none() {
        return Err(ErrorData::invalid_request(
            "interactive tools require the client's form elicitation capability",
            None,
        ));
    }
    Ok(())
}

pub(crate) fn new_request_state_codec() -> RequestStateCodec {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    RequestStateCodec::new(key)
}

pub(crate) fn seal_request_state(
    codec: &RequestStateCodec,
    tool_name: &str,
    arguments: Option<&Map<String, Value>>,
) -> String {
    let associated_data = associated_data(tool_name, arguments);
    codec.seal_with(STATE_PAYLOAD, &SealOptions::new().associated_data(&associated_data).ttl(STATE_TTL))
}

pub(crate) fn verify_request_state(
    codec: &RequestStateCodec,
    state: &str,
    tool_name: &str,
    arguments: Option<&Map<String, Value>>,
) -> Result<(), ErrorData> {
    let associated_data = associated_data(tool_name, arguments);
    codec
        .open_with(state, &associated_data)
        .and_then(|payload| {
            (payload == STATE_PAYLOAD).then_some(()).ok_or(rmcp::model::RequestStateError::MalformedFormat)
        })
        .map_err(|error| ErrorData::invalid_params(format!("invalid MRTR requestState: {error}"), None))
}

pub(crate) fn input_required(key: &str, request: ElicitRequestParams, state: String) -> CallToolResponse {
    let mut requests = InputRequests::new();
    requests.insert(key.to_string(), InputRequest::Elicitation(ElicitRequest::new(request)));
    CallToolResponse::InputRequired(InputRequiredResult::new(Some(requests), Some(state)))
}

pub(crate) fn elicitation_result(
    responses: Option<&rmcp::model::InputResponses>,
    key: &str,
) -> Result<ElicitResult, ErrorData> {
    let responses = responses.ok_or_else(|| ErrorData::invalid_params("MRTR retry is missing inputResponses", None))?;
    let value = responses
        .get(key)
        .ok_or_else(|| ErrorData::invalid_params(format!("MRTR retry is missing inputResponses[{key}]"), None))?;
    serde_json::from_value(value.clone())
        .map_err(|error| ErrorData::invalid_params(format!("MRTR response for {key} is malformed: {error}"), None))
}

fn associated_data(tool_name: &str, arguments: Option<&Map<String, Value>>) -> Vec<u8> {
    let value = serde_json::json!({
        "method": "tools/call",
        "tool": tool_name,
        "arguments": arguments.map_or(Value::Null, |args| canonicalize(Value::Object(args.clone()))),
    });
    serde_json::to_vec(&value).expect("JSON associated data is always serializable")
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sorted = Map::new();
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                sorted.insert(key, canonicalize(value));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_binds_tool_and_arguments_without_readable_arguments() {
        let codec = new_request_state_codec();
        let args = serde_json::from_value::<Map<String, Value>>(
            serde_json::json!({"secret": "value", "nested": {"b": 2, "a": 1}}),
        )
        .unwrap();
        let state = seal_request_state(&codec, "ask_user", Some(&args));
        assert!(!state.contains("secret"));
        assert!(verify_request_state(&codec, &state, "ask_user", Some(&args)).is_ok());
        assert!(verify_request_state(&codec, &state, "other", Some(&args)).is_err());
        let changed = serde_json::from_value::<Map<String, Value>>(
            serde_json::json!({"secret": "changed", "nested": {"a": 1, "b": 2}}),
        )
        .unwrap();
        assert!(verify_request_state(&codec, &state, "ask_user", Some(&changed)).is_err());
    }

    #[test]
    fn extra_response_keys_are_ignored() {
        let mut responses = rmcp::model::InputResponses::new();
        responses.insert("form".into(), serde_json::json!({"action": "decline"}));
        responses.insert("unrelated".into(), serde_json::json!("ignored"));
        assert_eq!(
            elicitation_result(Some(&responses), "form").unwrap().action,
            rmcp::model::ElicitationAction::Decline
        );
    }
}
