use rmcp::RoleServer;
use rmcp::model::{ClientCapabilities, ErrorData, InputResponses};
use rmcp::service::{ElicitationMode, RequestContext};
use serde::de::DeserializeOwned;

#[cfg(feature = "coding")]
pub(crate) const BACKGROUND_TASK_TTL_MS: u64 = 3_600_000;

#[cfg(feature = "coding")]
pub(crate) fn require_tasks(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    if context.client_capabilities().is_some_and(|capabilities| capabilities.supports_tasks()) {
        Ok(())
    } else {
        Err(ErrorData::missing_required_client_capability(ClientCapabilities::builder().enable_tasks().build()))
    }
}

pub(crate) fn supports_elicitation(context: &RequestContext<RoleServer>, mode: ElicitationMode) -> bool {
    context.client_capabilities().is_some_and(|capabilities| supports_elicitation_mode(&capabilities, mode))
}

pub(crate) fn parse_response<T: DeserializeOwned>(responses: &InputResponses, key: &str) -> Result<T, ErrorData> {
    let response = responses
        .get(key)
        .ok_or_else(|| ErrorData::invalid_params(format!("missing input response for '{key}'"), None))?;

    serde_json::from_value(response.clone())
        .map_err(|e| ErrorData::invalid_params(format!("invalid input response for '{key}': {e}"), None))
}

fn supports_elicitation_mode(capabilities: &ClientCapabilities, mode: ElicitationMode) -> bool {
    let Some(elicitation) = capabilities.elicitation.as_ref() else {
        return false;
    };
    match mode {
        ElicitationMode::Form => elicitation.form.is_some() || elicitation.url.is_none(),
        ElicitationMode::Url => elicitation.url.is_some(),
        _ => false,
    }
}
