//! JSON encodings for the content-carrying `GenAI` span attributes
//! (`gen_ai.system_instructions`, `gen_ai.input.messages`,
//! `gen_ai.output.messages`, `gen_ai.tool.definitions`).

use llm::{ToolCallRequest, ToolDefinition};
use serde_json::{Value, from_str, json};

pub(crate) fn system_instructions_json(content: &str) -> String {
    json!([{ "type": "text", "content": content }]).to_string()
}

pub(crate) fn input_messages_json(content: &str) -> String {
    json!([
        {
            "role": "user",
            "parts": [{ "type": "text", "content": content }]
        }
    ])
    .to_string()
}

pub(crate) fn output_messages_json(
    content: Option<&str>,
    tool_calls: &[ToolCallRequest],
    finish_reason: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(content) = content {
        parts.push(json!({ "type": "text", "content": content }));
    }

    parts.extend(tool_calls.iter().map(|tool_call| {
        let arguments = from_str(&tool_call.arguments).unwrap_or_else(|_| Value::String(tool_call.arguments.clone()));
        json!({
            "type": "tool_call",
            "id": tool_call.id,
            "name": tool_call.name,
            "arguments": arguments,
        })
    }));

    (!parts.is_empty()).then(|| {
        let mut message = json!({ "role": "assistant", "parts": parts });
        if let Some(finish_reason) = finish_reason {
            message["finish_reason"] = finish_reason.into();
        }
        Value::Array(vec![message]).to_string()
    })
}

pub(crate) fn tool_definitions_json(tools: &[ToolDefinition]) -> String {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                let parameters = tool.parameters.clone();
                json!({
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
