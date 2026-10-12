use rmcp::RoleServer;
use rmcp::model::{ErrorData, InputResponses};
use rmcp::service::{ElicitationMode, RequestContext};
use serde::de::DeserializeOwned;

pub(crate) const CLIENT_CANNOT_ELICIT: &str =
    "the connected client cannot prompt the user (MCP elicitation over protocol 2026-07-28 or newer)";

pub(crate) fn supports_elicitation(context: &RequestContext<RoleServer>, mode: ElicitationMode) -> bool {
    let Some(elicitation) = context.client_capabilities().and_then(|capabilities| capabilities.elicitation) else {
        return false;
    };
    match mode {
        ElicitationMode::Form => elicitation.form.is_some() || elicitation.url.is_none(),
        ElicitationMode::Url => elicitation.url.is_some(),
        _ => false,
    }
}

#[cfg(feature = "coding")]
pub(crate) fn require_tasks(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    if context.client_capabilities().is_some_and(|capabilities| capabilities.supports_tasks()) {
        Ok(())
    } else {
        let tasks = rmcp::model::ClientCapabilities::builder().enable_tasks().build();
        Err(ErrorData::missing_required_client_capability(tasks))
    }
}

pub(crate) fn parse_response<T: DeserializeOwned>(responses: &InputResponses, key: &str) -> Result<T, ErrorData> {
    let response = responses
        .get(key)
        .ok_or_else(|| ErrorData::invalid_params(format!("missing input response for '{key}'"), None))?;

    serde_json::from_value(response.clone())
        .map_err(|e| ErrorData::invalid_params(format!("invalid input response for '{key}': {e}"), None))
}
