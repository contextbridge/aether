use super::types::{
    CAGenerateContentRequest, Content, FunctionCall, FunctionDeclaration, FunctionResponse,
    GenerationConfig, Part, SYNTHETIC_THOUGHT_SIGNATURE, Tool, VertexGenerateContentRequest,
};
use crate::llm::{ChatMessage, Context, ToolDefinition};
use uuid::Uuid;

/// Build a CodeAssist API request from aether's Context
pub fn build_codeassist_request(model: &str, context: &Context) -> CAGenerateContentRequest {
    let (system_instruction, contents) = map_messages_to_contents(context.messages());
    let tools = map_tools(context.tools());

    CAGenerateContentRequest {
        model: model.to_string(),
        project: None,
        user_prompt_id: Some(Uuid::new_v4().to_string()),
        request: VertexGenerateContentRequest {
            contents,
            system_instruction,
            tools,
            tool_config: None,
            generation_config: Some(GenerationConfig {
                max_output_tokens: Some(16384),
                ..Default::default()
            }),
        },
    }
}

/// Map ChatMessage list to CodeAssist Content list, extracting system instruction
fn map_messages_to_contents(messages: &[ChatMessage]) -> (Option<Content>, Vec<Content>) {
    let mut system_instruction = None;
    let mut contents = Vec::new();
    let mut pending_tool_results: Vec<Part> = Vec::new();

    for msg in messages {
        // Before processing non-tool-result messages, flush any pending tool results
        if !matches!(msg, ChatMessage::ToolCallResult(_)) && !pending_tool_results.is_empty() {
            contents.push(Content {
                role: "user".to_string(),
                parts: std::mem::take(&mut pending_tool_results),
            });
        }

        match msg {
            ChatMessage::System { content, .. } => {
                // System instruction uses "user" role in Gemini format
                system_instruction = Some(Content {
                    role: "user".to_string(),
                    parts: vec![Part::Text {
                        text: content.clone(),
                    }],
                });
            }
            ChatMessage::User { content, .. } => {
                contents.push(Content {
                    role: "user".to_string(),
                    parts: vec![Part::Text {
                        text: content.clone(),
                    }],
                });
            }
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let mut parts = Vec::new();

                // Add text content if present
                if !content.is_empty() {
                    parts.push(Part::Text {
                        text: content.clone(),
                    });
                }

                // Add function calls with synthetic thought signature
                // (required by Gemini for function calls in conversation history)
                let mut first_call = true;
                for call in tool_calls {
                    let args = serde_json::from_str(&call.arguments).unwrap_or_default();
                    // Only the first function call in each model turn needs the signature
                    let thought_signature = if first_call {
                        first_call = false;
                        Some(SYNTHETIC_THOUGHT_SIGNATURE.to_string())
                    } else {
                        None
                    };
                    parts.push(Part::FunctionCall {
                        function_call: FunctionCall {
                            name: call.name.clone(),
                            args,
                        },
                        thought_signature,
                    });
                }

                if !parts.is_empty() {
                    contents.push(Content {
                        role: "model".to_string(),
                        parts,
                    });
                }
            }
            ChatMessage::ToolCallResult(result) => {
                // Accumulate tool results - they'll be flushed when we see a non-tool-result message
                let part = match result {
                    Ok(tool_result) => {
                        let response = serde_json::from_str(&tool_result.result).unwrap_or_else(
                            |_| serde_json::json!({ "result": tool_result.result }),
                        );
                        Part::FunctionResponse {
                            function_response: FunctionResponse {
                                name: tool_result.name.clone(),
                                response,
                            },
                        }
                    }
                    Err(tool_error) => Part::FunctionResponse {
                        function_response: FunctionResponse {
                            name: tool_error.name.clone(),
                            response: serde_json::json!({ "error": tool_error.error }),
                        },
                    },
                };
                pending_tool_results.push(part);
            }
            ChatMessage::Error { .. } => {
                // Skip error messages in the context
            }
            ChatMessage::Summary { content, .. } => {
                // Summaries from previous compaction are treated as user context
                contents.push(Content {
                    role: "user".to_string(),
                    parts: vec![Part::Text {
                        text: format!("[Previous conversation summary]\n\n{content}"),
                    }],
                });
            }
        }
    }

    // Flush any remaining tool results at the end
    if !pending_tool_results.is_empty() {
        contents.push(Content {
            role: "user".to_string(),
            parts: pending_tool_results,
        });
    }

    (system_instruction, contents)
}

/// Map ToolDefinition list to CodeAssist Tool format
fn map_tools(tools: &[ToolDefinition]) -> Option<Vec<Tool>> {
    if tools.is_empty() {
        return None;
    }

    let declarations: Vec<FunctionDeclaration> = tools
        .iter()
        .map(|tool| {
            let parameters =
                serde_json::from_str(&tool.parameters).unwrap_or(serde_json::json!({}));
            // Sanitize the schema to remove unsupported JSON Schema features
            let sanitized = sanitize_schema_for_gemini(parameters);
            FunctionDeclaration {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: sanitized,
            }
        })
        .collect();

    Some(vec![Tool {
        function_declarations: declarations,
    }])
}

/// Sanitize a JSON Schema to remove fields not supported by Gemini's API.
///
/// Gemini uses a simplified schema format that doesn't support:
/// - `$schema`, `$defs`, `$ref` (JSON Schema meta-fields)
/// - `const` (use enum with single value instead)
/// - `anyOf`, `oneOf`, `allOf` (complex union types)
fn sanitize_schema_for_gemini(schema: serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(mut obj) => {
            // Remove unsupported JSON Schema fields (both camelCase and snake_case variants)
            const UNSUPPORTED_FIELDS: &[&str] = &[
                "$schema",
                "$defs",
                "$ref",
                "$id",
                "const",
                "anyOf",
                "any_of",
                "oneOf",
                "one_of",
                "allOf",
                "all_of",
                "if",
                "then",
                "else",
                "not",
                "additionalProperties",
                "additional_properties",
                "patternProperties",
                "pattern_properties",
                "unevaluatedProperties",
                "unevaluated_properties",
                "propertyNames",
                "property_names",
                "minProperties",
                "min_properties",
                "maxProperties",
                "max_properties",
                "dependentRequired",
                "dependent_required",
                "dependentSchemas",
                "dependent_schemas",
            ];

            for field in UNSUPPORTED_FIELDS {
                obj.remove(*field);
            }

            // Recursively sanitize nested objects
            let sanitized: serde_json::Map<String, serde_json::Value> = obj
                .into_iter()
                .map(|(k, v)| (k, sanitize_schema_for_gemini(v)))
                .collect();

            serde_json::Value::Object(sanitized)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sanitize_schema_for_gemini).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCallRequest, ToolCallResult};
    use crate::types::IsoString;

    fn timestamp() -> IsoString {
        IsoString::now()
    }

    #[test]
    fn test_map_user_message() {
        let messages = vec![ChatMessage::User {
            content: "Hello".to_string(),
            timestamp: timestamp(),
        }];

        let (system, contents) = map_messages_to_contents(&messages);

        assert!(system.is_none());
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, "user");

        match &contents[0].parts[0] {
            Part::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn test_map_system_message() {
        let messages = vec![ChatMessage::System {
            content: "You are helpful".to_string(),
            timestamp: timestamp(),
        }];

        let (system, contents) = map_messages_to_contents(&messages);

        assert!(system.is_some());
        assert!(contents.is_empty());

        let sys = system.unwrap();
        assert_eq!(sys.role, "user");
        match &sys.parts[0] {
            Part::Text { text } => assert_eq!(text, "You are helpful"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn test_map_assistant_with_tool_call() {
        let messages = vec![ChatMessage::Assistant {
            content: "".to_string(),
            timestamp: timestamp(),
            tool_calls: vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: r#"{"location":"NYC"}"#.to_string(),
            }],
        }];

        let (_, contents) = map_messages_to_contents(&messages);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, "model");

        match &contents[0].parts[0] {
            Part::FunctionCall {
                function_call,
                thought_signature,
            } => {
                assert_eq!(function_call.name, "get_weather");
                assert_eq!(function_call.args["location"], "NYC");
                // First function call should have the synthetic signature
                assert_eq!(
                    thought_signature.as_deref(),
                    Some(SYNTHETIC_THOUGHT_SIGNATURE)
                );
            }
            _ => panic!("Expected function call"),
        }
    }

    #[test]
    fn test_map_tool_result() {
        let messages = vec![ChatMessage::ToolCallResult(Ok(ToolCallResult {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"location":"NYC"}"#.to_string(),
            result: r#"{"temp": 72}"#.to_string(),
        }))];

        let (_, contents) = map_messages_to_contents(&messages);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, "user");

        match &contents[0].parts[0] {
            Part::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "get_weather");
                assert_eq!(function_response.response["temp"], 72);
            }
            _ => panic!("Expected function response"),
        }
    }

    #[test]
    fn test_map_tools() {
        let tools = vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather for a location".to_string(),
            parameters: r#"{"type":"object","properties":{"location":{"type":"string"}}}"#
                .to_string(),
            server: None,
        }];

        let result = map_tools(&tools);

        assert!(result.is_some());
        let tool_list = result.unwrap();
        assert_eq!(tool_list.len(), 1);

        let decl = &tool_list[0].function_declarations[0];
        assert_eq!(decl.name, "get_weather");
        assert_eq!(decl.description, "Get weather for a location");
    }

    #[test]
    fn test_build_full_request() {
        let context = Context::new(
            vec![
                ChatMessage::System {
                    content: "Be helpful".to_string(),
                    timestamp: timestamp(),
                },
                ChatMessage::User {
                    content: "Hi".to_string(),
                    timestamp: timestamp(),
                },
            ],
            vec![],
        );

        let request = build_codeassist_request("gemini-2.0-flash", &context);

        assert_eq!(request.model, "gemini-2.0-flash");
        assert!(request.user_prompt_id.is_some());
        assert!(request.request.system_instruction.is_some());
        assert_eq!(request.request.contents.len(), 1);
    }
}
