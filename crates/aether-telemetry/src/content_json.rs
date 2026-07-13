//! JSON encodings for the content-carrying `GenAI` span attributes
//! (`gen_ai.input.messages`, `gen_ai.output.messages`, `gen_ai.tool.definitions`).

use llm::ToolDefinition;

pub(crate) fn messages_json(role: &str, content: &str) -> String {
    serde_json::json!([
        {
            "role": role,
            "parts": [{ "type": "text", "content": content }]
        }
    ])
    .to_string()
}

pub(crate) fn tool_definitions_json(tools: &[ToolDefinition]) -> String {
    serde_json::Value::Array(
        tools
            .iter()
            .map(|tool| {
                let parameters = tool.parameters.clone();
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": parameters,
                })
            })
            .collect(),
    )
    .to_string()
}
