use schemars::Schema;

use super::types::{
    InputContent, InputItem, MessageContent, Reasoning, ReasoningContent, ResponsesRequest, Role, TextFormat,
    TextParam, Tool,
};
use crate::providers::openai_compatible::{PromptCacheKeySource, reasoning_effort_param};
use crate::tool_schema::normalize_for_xiaomi;
use crate::{
    ChatMessage, ContentBlock, Context, LlmError, LlmModel, ReasoningEffort, Result, ToolDefinition, tool_result_parts,
};

/// The per-provider decisions that shape an otherwise identical Responses request.
pub struct ResponsesRequestPolicy {
    /// Send `reasoning` even when no effort was requested.
    always_include_reasoning: bool,
    /// Effort applied when the caller did not request one.
    default_effort: Option<ReasoningEffort>,
    /// Wire value for `ReasoningEffort::Disabled`.
    disabled_effort: &'static str,
    text_verbosity: Option<&'static str>,
    /// `strict` sent on every function tool. The Responses API defaults this to
    /// `true`, which rejects tool schemas that omit `additionalProperties`, so
    /// `None` and `Some(false)` are not interchangeable.
    tool_strict: Option<bool>,
    parallel_tool_calls: Option<bool>,
    tool_schema_transform: Option<fn(&mut Schema)>,
    reasoning_format: ReasoningFormat,
    prompt_cache_key: PromptCacheKeySource,
}

impl ResponsesRequestPolicy {
    pub const OPENAI: Self = Self {
        always_include_reasoning: false,
        default_effort: None,
        disabled_effort: "none",
        text_verbosity: None,
        tool_strict: Some(false),
        parallel_tool_calls: None,
        tool_schema_transform: None,
        reasoning_format: ReasoningFormat::Encrypted,
        prompt_cache_key: PromptCacheKeySource::Prefix,
    };

    pub const XIAOMI: Self = Self {
        tool_schema_transform: Some(normalize_for_xiaomi),
        reasoning_format: ReasoningFormat::PlainText,
        prompt_cache_key: PromptCacheKeySource::Omit,
        ..Self::OPENAI
    };

    #[cfg(feature = "codex")]
    pub const CODEX: Self = Self {
        always_include_reasoning: true,
        default_effort: Some(ReasoningEffort::Medium),
        disabled_effort: "disabled",
        text_verbosity: Some("medium"),
        tool_strict: None,
        parallel_tool_calls: Some(true),
        ..Self::OPENAI
    };

    #[cfg(feature = "bedrock")]
    pub const MANTLE: Self = Self { always_include_reasoning: true, tool_strict: None, ..Self::OPENAI };

    /// Effort to send, if any — an explicit request beats the provider default.
    fn effort(&self, context: &Context) -> Option<ReasoningEffort> {
        match context.reasoning_effort() {
            ReasoningEffort::Default => self.default_effort,
            effort => Some(effort),
        }
    }
}

/// How a provider exposes reasoning and expects it back on later turns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReasoningFormat {
    /// Opaque encrypted items replayed only to the model that produced them,
    /// alongside human-readable summaries.
    Encrypted,
    /// Raw reasoning text, streamed and replayed as-is with no summaries.
    PlainText,
}

pub(crate) fn build_request(
    identity: Option<&LlmModel>,
    wire_model: &str,
    context: &Context,
    policy: &ResponsesRequestPolicy,
) -> Result<ResponsesRequest> {
    let context = context.filter_encrypted_reasoning(identity);
    let encrypted = policy.reasoning_format == ReasoningFormat::Encrypted;
    let (instructions, input) = map_messages(context.messages(), policy.reasoning_format)?;
    let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), policy)?) };
    let settings = context.model_settings();
    let effort = policy.effort(&context);
    let reasoning = (policy.always_include_reasoning || effort.is_some()).then(|| Reasoning {
        summary: (encrypted && context.reasoning_effort() != ReasoningEffort::Disabled).then_some("auto"),
        effort: effort.and_then(|effort| match effort {
            ReasoningEffort::Disabled => Some(policy.disabled_effort),
            effort => reasoning_effort_param(effort),
        }),
    });

    Ok(ResponsesRequest {
        include: encrypted.then_some(&["reasoning.encrypted_content"][..]),
        input,
        instructions,
        max_output_tokens: settings.max_tokens,
        model: wire_model.to_string(),
        parallel_tool_calls: policy.parallel_tool_calls,
        prompt_cache_key: policy.prompt_cache_key.resolve(&context).map(String::from),
        reasoning,
        store: false,
        stream: true,
        temperature: settings.temperature,
        text: policy.text_verbosity.map(|verbosity| TextParam { format: TextFormat::Text, verbosity }),
        tools,
        top_p: settings.top_p,
    })
}

/// Map internal `ChatMessage`s to Responses API input items.
///
/// Returns `(system_prompt, input_items)` — the system prompt is extracted
/// separately since the Responses API carries it as `instructions`.
fn map_messages(
    messages: &[ChatMessage],
    reasoning_format: ReasoningFormat,
) -> Result<(Option<String>, Vec<InputItem>)> {
    let mut system_prompt = None;
    let mut items = Vec::new();

    for msg in messages {
        match msg {
            ChatMessage::System { content, .. } => {
                system_prompt = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                items.push(InputItem::Message { role: Role::User, content: map_user_content(content)? });
            }
            ChatMessage::Assistant { content, tool_calls, reasoning, .. } => {
                if !content.is_empty() {
                    items.push(text_message(Role::Assistant, content.clone()));
                }
                if reasoning_format == ReasoningFormat::PlainText
                    && let Some(text) = &reasoning.summary_text
                {
                    items.push(InputItem::Reasoning {
                        id: None,
                        summary: vec![],
                        encrypted_content: None,
                        content: Some(vec![ReasoningContent::ReasoningText { text: text.clone() }]),
                    });
                }
                if reasoning_format == ReasoningFormat::Encrypted
                    && let Some(encrypted) = &reasoning.encrypted_content
                {
                    items.push(InputItem::Reasoning {
                        id: Some(encrypted.id.clone()),
                        summary: vec![],
                        encrypted_content: Some(encrypted.content.clone()),
                        content: None,
                    });
                }
                items.extend(tool_calls.iter().map(|call| InputItem::FunctionCall {
                    arguments: call.arguments.clone(),
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                }));
            }
            ChatMessage::ToolCallResult(result) => {
                let (call_id, content, is_error) = tool_result_parts(result);
                let output = if is_error { format!("Error: {content}") } else { content.to_string() };
                items.push(InputItem::FunctionCallOutput { call_id: call_id.to_string(), output });
            }
            ChatMessage::Error { .. } | ChatMessage::Summary { .. } => {
                items.extend(msg.as_user_note().map(|note| text_message(Role::User, note)));
            }
        }
    }

    Ok((system_prompt, items))
}

fn map_user_content(parts: &[ContentBlock]) -> Result<MessageContent> {
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentBlock::Text { text } => items.push(InputContent::InputText { text: text.clone() }),
            ContentBlock::Image { .. } => items.push(InputContent::InputImage {
                detail: "auto",
                image_url: part.as_data_uri().expect("image content always has a data URI"),
            }),
            ContentBlock::Audio { .. } => {
                return Err(LlmError::UnsupportedContent("OpenAI Responses does not support audio input".into()));
            }
        }
    }
    Ok(MessageContent::Parts(items))
}

fn map_tools(tools: &[ToolDefinition], policy: &ResponsesRequestPolicy) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function {
                name: tool.name.clone(),
                parameters: tool.parameters_with(policy.tool_schema_transform)?,
                strict: policy.tool_strict,
                description: tool.description.clone(),
            })
        })
        .collect()
}

fn text_message(role: Role, content: String) -> InputItem {
    InputItem::Message { role, content: MessageContent::Text(content) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IsoString;
    use crate::{
        AssistantReasoning, EncryptedReasoningContent, MessageId, ToolCallError, ToolCallRequest, ToolCallResult,
    };
    use serde_json::{Value, json};

    fn openai_request(model: &str, context: &Context) -> Result<ResponsesRequest> {
        build_request(None, model, context, &ResponsesRequestPolicy::OPENAI)
    }

    fn openai_body(model: &str, context: &Context) -> Value {
        serde_json::to_value(openai_request(model, context).unwrap()).unwrap()
    }

    fn input_json(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
        let (system, items) = map_messages(messages, ReasoningFormat::Encrypted).unwrap();
        (system, items.iter().map(|item| serde_json::to_value(item).unwrap()).collect())
    }

    #[test]
    fn build_request_maps_a_simple_user_message() {
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let body = openai_body("gpt-4.1", &context);
        assert_eq!(body["model"], "gpt-4.1");
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("reasoning").is_none());
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn build_request_lifts_the_system_prompt_into_instructions() {
        let context = Context::new(vec![ChatMessage::system("You are helpful."), ChatMessage::user("Hi")], vec![]);

        let body = openai_body("gpt-4.1", &context);
        assert_eq!(body["instructions"], "You are helpful.");
        let items = body["input"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
    }

    #[test]
    fn build_request_maps_tool_calls_and_results() {
        let context = Context::new(
            vec![
                ChatMessage::user("Search for rust"),
                ChatMessage::Assistant {
                    message_id: crate::MessageId::new(),
                    content: String::new(),
                    reasoning: AssistantReasoning::default(),
                    timestamp: IsoString::now(),
                    tool_calls: vec![ToolCallRequest {
                        id: "call_1".to_string(),
                        name: "search".to_string(),
                        arguments: r#"{"q":"rust"}"#.to_string(),
                    }],
                },
                ChatMessage::ToolCallResult(Ok(ToolCallResult {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"q":"rust"}"#.to_string(),
                    result: "Found results".to_string(),
                })),
            ],
            vec![ToolDefinition::new("search", "Search", serde_json::json!({ "type": "object" }))],
        );

        let body = openai_body("gpt-4.1", &context);

        let items = body["input"].as_array().unwrap();
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_1");
        assert_eq!(items[2]["output"], "Found results");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "search");
    }

    #[test]
    fn build_request_applies_model_settings() {
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_model_settings(crate::ModelSettings {
            temperature: Some(0.0),
            top_p: Some(0.5),
            max_tokens: Some(128),
        });

        let body = openai_body("gpt-4.1", &context);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["top_p"], 0.5);
        assert_eq!(body["max_output_tokens"], 128);
    }

    #[test]
    fn build_request_rejects_audio_content() {
        let context = Context::new(
            vec![ChatMessage::User {
                message_id: MessageId::new(),
                content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        assert!(matches!(openai_request("gpt-4.1", &context), Err(LlmError::UnsupportedContent(_))));
    }

    #[test]
    fn wire_request_carries_every_reasoning_effort() {
        for effort in ReasoningEffort::all().iter().filter(|effort| effort.is_enabled()) {
            let mut context = Context::new(vec![ChatMessage::user("Think")], vec![]);
            context.set_reasoning_effort(*effort);

            let body = openai_body("gpt-5.6", &context);
            assert_eq!(body["reasoning"]["effort"], effort.as_str());
            assert_eq!(body["reasoning"]["summary"], "auto");
        }
    }

    #[test]
    fn wire_request_omits_reasoning_without_an_effort() {
        let context = Context::new(vec![ChatMessage::user("Hi")], vec![]);

        assert!(openai_body("gpt-4.1", &context)["reasoning"].is_null());
    }

    #[cfg(feature = "codex")]
    #[test]
    fn a_provider_default_effort_always_ships_with_a_reasoning_object() {
        let context = Context::new(vec![ChatMessage::user("Hi")], vec![]);

        let request = build_request(None, "gpt-5.5", &context, &ResponsesRequestPolicy::CODEX).unwrap();
        let body = serde_json::to_value(request).unwrap();

        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    #[cfg(feature = "codex")]
    #[test]
    fn tool_strict_is_sent_verbatim_per_policy() {
        let tools = vec![ToolDefinition::new("read_file", "Read a file", serde_json::json!({ "type": "object" }))];

        let openai = serde_json::to_value(map_tools(&tools, &ResponsesRequestPolicy::OPENAI).unwrap()).unwrap();
        let codex = serde_json::to_value(map_tools(&tools, &ResponsesRequestPolicy::CODEX).unwrap()).unwrap();

        assert_eq!(openai[0]["strict"], false);
        assert!(codex[0].get("strict").is_none(), "{codex}");
    }

    #[test]
    fn map_messages_extracts_system_prompt() {
        let messages = vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")];

        let (system, items) = input_json(&messages);
        assert_eq!(system, Some("You are helpful".to_string()));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn map_messages_handles_multi_turn_with_tool_calls() {
        let messages = vec![
            ChatMessage::user("Read foo.rs"),
            ChatMessage::Assistant {
                message_id: MessageId::new(),
                content: "I'll read that file.".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"foo.rs"}"#.to_string(),
                }],
            },
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"foo.rs"}"#.to_string(),
                result: "fn main() {}".to_string(),
            })),
            ChatMessage::Assistant {
                message_id: MessageId::new(),
                content: "Here's the file content.".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![],
            },
        ];

        let (system, items) = input_json(&messages);
        assert!(system.is_none());
        assert_eq!(items.len(), 5); // user + assistant msg + function_call + function_call_output + assistant msg
        assert_eq!(
            items[2],
            json!({ "type": "function_call", "arguments": r#"{"path":"foo.rs"}"#, "call_id": "call_1", "name": "read_file" })
        );
        assert_eq!(items[3], json!({ "type": "function_call_output", "call_id": "call_1", "output": "fn main() {}" }));
    }

    #[test]
    fn map_messages_handles_tool_errors() {
        let messages = vec![ChatMessage::ToolCallResult(Err(ToolCallError {
            id: "call_2".to_string(),
            name: "bash".to_string(),
            arguments: Some("{}".to_string()),
            error: "command failed".to_string(),
        }))];

        let (_, items) = input_json(&messages);
        assert_eq!(
            items,
            vec![json!({ "type": "function_call_output", "call_id": "call_2", "output": "Error: command failed" })]
        );
    }

    #[test]
    fn map_messages_handles_summary() {
        let messages = vec![ChatMessage::Summary {
            message_id: MessageId::new(),
            content: "User asked about Rust.".to_string(),
            timestamp: IsoString::now(),
            messages_compacted: 5,
        }];

        let (_, items) = input_json(&messages);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");
        let text = items[0]["content"].as_str().expect("summary is sent as plain text");
        assert!(text.starts_with("[Previous conversation handoff]"));
        assert!(text.contains("Rust"));
    }

    #[test]
    fn map_tools_produces_function_type() {
        let tools = vec![ToolDefinition::new(
            "read_file",
            "Read a file from disk",
            serde_json::from_str(r#"{"type": "object", "properties": {"path": {"type": "string"}}}"#).unwrap(),
        )];

        let mapped = serde_json::to_value(map_tools(&tools, &ResponsesRequestPolicy::OPENAI).unwrap()).unwrap();
        assert_eq!(mapped.as_array().map(Vec::len), Some(1));
        assert_eq!(mapped[0]["type"], "function");
        assert_eq!(mapped[0]["name"], "read_file");
        assert_eq!(mapped[0]["description"], "Read a file from disk");
        assert_eq!(mapped[0]["parameters"]["properties"]["path"]["type"], "string");
    }

    #[test]
    fn map_messages_includes_encrypted_reasoning_item() {
        let messages = vec![ChatMessage::Assistant {
            message_id: MessageId::new(),
            content: "thinking done".to_string(),
            reasoning: AssistantReasoning::from_parts(
                "summary".to_string(),
                Some(EncryptedReasoningContent {
                    id: "r_1".to_string(),
                    model: crate::LlmModel::Ollama("test".to_string()),
                    content: "encrypted-blob".to_string(),
                }),
            ),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = input_json(&messages);
        // Should have: text message + reasoning item = 2
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[1],
            json!({ "type": "reasoning", "id": "r_1", "summary": [], "encrypted_content": "encrypted-blob" })
        );
    }

    #[test]
    fn map_messages_skips_reasoning_item_without_encrypted_content() {
        let messages = vec![ChatMessage::Assistant {
            message_id: MessageId::new(),
            content: "no encrypted".to_string(),
            reasoning: AssistantReasoning::from_parts("just a summary".to_string(), None),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = input_json(&messages);
        // Only the text message, no reasoning item
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
    }

    #[test]
    fn map_messages_with_audio_errors() {
        let messages = vec![ChatMessage::User {
            message_id: MessageId::new(),
            content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
            timestamp: IsoString::now(),
        }];

        assert!(matches!(map_messages(&messages, ReasoningFormat::Encrypted), Err(LlmError::UnsupportedContent(_))));
    }
}
