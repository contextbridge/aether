use rmcp::model::{CallToolRequestParams, ErrorData};
use serde::de::DeserializeOwned;

/// Deserialize a tool call's arguments into a typed input
pub fn parse_arguments<T: DeserializeOwned>(request: &CallToolRequestParams) -> Result<T, ErrorData> {
    let arguments = request.arguments.clone().unwrap_or_default();
    serde_json::from_value(serde_json::Value::Object(arguments))
        .map_err(|e| ErrorData::invalid_params(format!("invalid arguments: {e}"), None))
}
