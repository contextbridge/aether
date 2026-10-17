use rmcp::ErrorData;
use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::{CallToolResponse, CallToolResult};
use serde::Serialize;
use utils::display_meta::ToolResultMeta;

#[derive(Debug)]
pub struct Displayed<T> {
    pub output: T,
    pub meta: Option<ToolResultMeta>,
}

impl<T> Displayed<T> {
    pub fn new(output: T, meta: impl Into<ToolResultMeta>) -> Self {
        Self { output, meta: Some(meta.into()) }
    }
}

impl<T: Serialize> Displayed<T> {
    pub fn into_result(self) -> Result<CallToolResult, serde_json::Error> {
        let mut result = CallToolResult::structured(serde_json::to_value(self.output)?);
        result.meta = self.meta.map(|meta| serde_json::to_value(meta).and_then(serde_json::from_value)).transpose()?;
        Ok(result)
    }
}

impl<T: Serialize> IntoCallToolResult for Displayed<T> {
    fn into_call_tool_result(self) -> Result<CallToolResponse, ErrorData> {
        self.into_result()
            .map(Into::into)
            .map_err(|error| ErrorData::internal_error(format!("Failed to serialize tool result: {error}"), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use utils::display_meta::ToolDisplayMeta;

    #[derive(Serialize)]
    struct Output {
        lines: u32,
    }

    #[test]
    fn output_is_structured_content_and_meta_is_the_result_meta() {
        let result = Displayed::new(Output { lines: 3 }, ToolDisplayMeta::new("Read file", "main.rs")).into_result();

        let result = result.unwrap();
        assert_eq!(result.structured_content, Some(json!({ "lines": 3 })));
        let meta = result.meta.expect("result meta");
        assert_eq!(meta.0["display"], json!({ "title": "Read file", "value": "main.rs" }));
    }
}
