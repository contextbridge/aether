use async_openai::types::responses::{
    CreateResponse, EasyInputContent, EasyInputMessage, FunctionCallOutput, FunctionCallOutputItemParam, FunctionTool,
    FunctionToolCall, ImageDetail, IncludeEnum, InputContent, InputImageContent, InputItem, InputParam,
    InputTextContent, Item, MessageType, Reasoning, ReasoningItem, ReasoningSummary, ResponseTextParam, Role,
    TextResponseFormatConfiguration, Tool, Verbosity,
};

use crate::catalog::Provider;
use crate::{ChatMessage, ContentBlock, Context, LlmError, LlmModel, ReasoningEffort, Result, ToolDefinition};

/// The per-provider decisions that shape an otherwise identical Responses request.
pub(crate) struct ResponsesRequestPolicy {
    /// Provider that owns the model — decides whose encrypted reasoning may be
    /// replayed from earlier turns.
    pub provider: Provider,
    /// Send `reasoning` even when no effort was requested.
    pub always_include_reasoning: bool,
    /// Effort applied when the caller did not request one.
    pub default_effort: Option<ReasoningEffort>,
    pub text_verbosity: Option<Verbosity>,
    /// `strict` sent on every function tool. The Responses API defaults this to
    /// `true`, which rejects tool schemas that omit `additionalProperties`, so
    /// `None` and `Some(false)` are not interchangeable.
    pub tool_strict: Option<bool>,
}

impl ResponsesRequestPolicy {
    pub const fn openai() -> Self {
        Self {
            provider: Provider::Openai,
            always_include_reasoning: false,
            default_effort: None,
            text_verbosity: None,
            tool_strict: Some(false),
        }
    }

    pub const fn codex() -> Self {
        Self {
            provider: Provider::Codex,
            always_include_reasoning: true,
            default_effort: Some(ReasoningEffort::Medium),
            text_verbosity: Some(Verbosity::Medium),
            tool_strict: None,
        }
    }

    pub const fn mantle() -> Self {
        Self {
            provider: Provider::Bedrock,
            always_include_reasoning: true,
            default_effort: None,
            text_verbosity: None,
            tool_strict: None,
        }
    }

    /// Effort to send, if any — an explicit request beats the provider default.
    fn effort(&self, context: &Context) -> Option<ReasoningEffort> {
        context.reasoning_effort().or(self.default_effort)
    }
}

pub(crate) fn build_typed_request(
    model: &str,
    context: &Context,
    policy: &ResponsesRequestPolicy,
) -> Result<CreateResponse> {
    let identity: Option<LlmModel> = format!("{}:{model}", policy.provider.parser_name()).parse().ok();
    let context = context.filter_encrypted_reasoning(identity.as_ref());
    let (instructions, input) = map_messages(context.messages())?;
    let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), policy.tool_strict)?) };
    let settings = context.model_settings();
    let reasoning = (policy.always_include_reasoning || policy.effort(&context).is_some())
        .then_some(Reasoning { effort: None, summary: Some(ReasoningSummary::Auto) });

    let text = policy.text_verbosity.clone().map(|verbosity| ResponseTextParam {
        format: TextResponseFormatConfiguration::Text,
        verbosity: Some(verbosity),
    });

    Ok(CreateResponse {
        model: Some(model.to_string()),
        input: InputParam::Items(input),
        instructions,
        tools,
        stream: Some(true),
        store: Some(false),
        max_output_tokens: settings.max_tokens,
        temperature: settings.temperature,
        top_p: settings.top_p,
        reasoning,
        include: Some(vec![IncludeEnum::ReasoningEncryptedContent]),
        text,
        prompt_cache_key: context.prompt_cache_key().map(String::from),
        ..Default::default()
    })
}

pub(crate) fn build_wire_request(
    model: &str,
    context: &Context,
    policy: &ResponsesRequestPolicy,
) -> Result<serde_json::Value> {
    let effort = policy.effort(context);
    let mut body = serde_json::to_value(build_typed_request(model, context, policy)?)?;

    // async-openai's `ReasoningEffort` cannot express `max`, so the effort is
    // written onto the serialized body instead of the typed request. Safe because
    // `build_typed_request` emits `reasoning` whenever `policy.effort` is set.
    if let Some(effort) = effort {
        body["reasoning"]["effort"] = effort.as_str().into();
    }
    Ok(body)
}

pub(crate) fn map_user_content_for_responses(parts: &[ContentBlock]) -> Result<EasyInputContent> {
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentBlock::Text { text } => {
                items.push(InputContent::InputText(InputTextContent { text: text.clone() }));
            }
            ContentBlock::Image { .. } => {
                items.push(InputContent::InputImage(InputImageContent {
                    detail: ImageDetail::Auto,
                    file_id: None,
                    image_url: Some(part.as_data_uri().expect("image content always has a data URI")),
                }));
            }
            ContentBlock::Audio { .. } => {
                return Err(LlmError::UnsupportedContent("OpenAI Responses does not support audio input".into()));
            }
        }
    }
    Ok(EasyInputContent::ContentList(items))
}

/// Map internal `ChatMessage`s to Responses API input items.
///
/// Returns `(system_prompt, input_items)` — the system prompt is extracted
/// separately since the Responses API carries it as `instructions`.
pub(crate) fn map_messages(messages: &[ChatMessage]) -> Result<(Option<String>, Vec<InputItem>)> {
    let mut system_prompt = None;
    let mut items = Vec::new();

    for msg in messages {
        match msg {
            ChatMessage::System { content, .. } => {
                system_prompt = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                items.push(InputItem::EasyMessage(EasyInputMessage {
                    r#type: MessageType::Message,
                    role: Role::User,
                    content: map_user_content_for_responses(content)?,
                    phase: None,
                }));
            }
            ChatMessage::Assistant { content, tool_calls, reasoning, .. } => {
                if !content.is_empty() {
                    items.push(easy_message(Role::Assistant, content.clone()));
                }
                if let Some(encrypted) = &reasoning.encrypted_content {
                    items.push(InputItem::Item(Item::Reasoning(ReasoningItem {
                        id: Some(encrypted.id.clone()),
                        summary: vec![],
                        encrypted_content: Some(encrypted.content.clone()),
                        content: None,
                        status: None,
                    })));
                }
                for tc in tool_calls {
                    items.push(InputItem::Item(Item::FunctionCall(FunctionToolCall {
                        call_id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                        namespace: None,
                        id: None,
                        status: None,
                    })));
                }
            }
            ChatMessage::ToolCallResult(result) => match result {
                Ok(r) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: r.id.clone(),
                        output: FunctionCallOutput::Text(r.result.clone()),
                        id: None,
                        status: None,
                    })));
                }
                Err(e) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: e.id.clone(),
                        output: FunctionCallOutput::Text(format!("Error: {}", e.error)),
                        id: None,
                        status: None,
                    })));
                }
            },
            ChatMessage::Error { message, .. } => {
                items.push(easy_message(Role::User, format!("[Error: {message}]")));
            }
            ChatMessage::Summary { content, .. } => {
                items.push(easy_message(Role::User, format!("[Summary of previous conversation]\n{content}")));
            }
        }
    }

    Ok((system_prompt, items))
}

/// Map internal `ToolDefinition`s to async-openai `Tool` types.
pub(crate) fn map_tools(tools: &[ToolDefinition], strict: Option<bool>) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function(FunctionTool {
                name: tool.name.clone(),
                description: Some(tool.description.clone()),
                parameters: Some(tool.parameters.clone()),
                strict,
                defer_loading: None,
            }))
        })
        .collect()
}

fn easy_message(role: Role, content: String) -> InputItem {
    InputItem::EasyMessage(EasyInputMessage { role, content: EasyInputContent::Text(content), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IsoString;
    use crate::{
        AssistantReasoning, ContentBlock, EncryptedReasoningContent, LlmError, ToolCallError, ToolCallRequest,
        ToolCallResult,
    };

    fn openai_request(model: &str, context: &Context) -> Result<CreateResponse> {
        build_typed_request(model, context, &ResponsesRequestPolicy::openai())
    }

    fn openai_body(model: &str, context: &Context) -> serde_json::Value {
        build_wire_request(model, context, &ResponsesRequestPolicy::openai()).unwrap()
    }

    #[test]
    fn build_request_maps_a_simple_user_message() {
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let request = openai_request("gpt-4.1", &context).unwrap();
        assert_eq!(request.model, Some("gpt-4.1".to_string()));
        assert!(request.instructions.is_none());
        assert!(request.tools.is_none());
        assert!(request.reasoning.is_none());

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn build_request_lifts_the_system_prompt_into_instructions() {
        let context = Context::new(vec![ChatMessage::system("You are helpful."), ChatMessage::user("Hi")], vec![]);

        let request = openai_request("gpt-4.1", &context).unwrap();
        assert_eq!(request.instructions, Some("You are helpful.".to_string()));

        let json = serde_json::to_value(&request).unwrap();
        let items = json["input"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
    }

    #[test]
    fn build_request_maps_tool_calls_and_results() {
        let context = Context::new(
            vec![
                ChatMessage::user("Search for rust"),
                ChatMessage::Assistant {
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

        let request = openai_request("gpt-4.1", &context).unwrap();
        let json = serde_json::to_value(&request).unwrap();

        let items = json["input"].as_array().unwrap();
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_1");
        assert_eq!(items[2]["output"], "Found results");

        let tools = serde_json::to_value(&request.tools).unwrap();
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "search");
    }

    #[test]
    fn build_request_applies_model_settings() {
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_model_settings(crate::ModelSettings {
            temperature: Some(0.0),
            top_p: Some(0.5),
            max_tokens: Some(128),
        });

        let request = openai_request("gpt-4.1", &context).unwrap();
        assert_eq!(request.temperature, Some(0.0));
        assert_eq!(request.top_p, Some(0.5));
        assert_eq!(request.max_output_tokens, Some(128));
    }

    #[test]
    fn build_request_rejects_audio_content() {
        let context = Context::new(
            vec![ChatMessage::User {
                content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        assert!(matches!(openai_request("gpt-4.1", &context), Err(LlmError::UnsupportedContent(_))));
    }

    #[test]
    fn wire_request_carries_every_reasoning_effort() {
        for effort in ReasoningEffort::all() {
            let mut context = Context::new(vec![ChatMessage::user("Think")], vec![]);
            context.set_reasoning_effort(Some(*effort));

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

    #[test]
    fn a_provider_default_effort_always_ships_with_a_reasoning_object() {
        let context = Context::new(vec![ChatMessage::user("Hi")], vec![]);

        let body = build_wire_request("gpt-5.5", &context, &ResponsesRequestPolicy::codex()).unwrap();

        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    #[test]
    fn tool_strict_is_sent_verbatim_per_policy() {
        let tools = vec![ToolDefinition::new("read_file", "Read a file", serde_json::json!({ "type": "object" }))];

        let openai =
            serde_json::to_value(map_tools(&tools, ResponsesRequestPolicy::openai().tool_strict).unwrap()).unwrap();
        let codex =
            serde_json::to_value(map_tools(&tools, ResponsesRequestPolicy::codex().tool_strict).unwrap()).unwrap();

        assert_eq!(openai[0]["strict"], false);
        assert!(codex[0].get("strict").is_none(), "{codex}");
    }

    #[test]
    fn map_messages_extracts_system_prompt() {
        let messages = vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")];

        let (system, items) = map_messages(&messages).unwrap();
        assert_eq!(system, Some("You are helpful".to_string()));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn map_messages_handles_multi_turn_with_tool_calls() {
        let messages = vec![
            ChatMessage::user("Read foo.rs"),
            ChatMessage::Assistant {
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
                content: "Here's the file content.".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![],
            },
        ];

        let (system, items) = map_messages(&messages).unwrap();
        assert!(system.is_none());
        assert_eq!(items.len(), 5); // user + assistant msg + function_call + function_call_output + assistant msg

        // Verify the function_call item
        let fc = &items[2];
        if let InputItem::Item(Item::FunctionCall(call)) = fc {
            assert_eq!(call.call_id, "call_1");
            assert_eq!(call.name, "read_file");
            assert_eq!(call.arguments, r#"{"path":"foo.rs"}"#);
        } else {
            panic!("Expected FunctionCall, got {fc:?}");
        }

        // Verify the function_call_output item
        let fco = &items[3];
        if let InputItem::Item(Item::FunctionCallOutput(out)) = fco {
            assert_eq!(out.call_id, "call_1");
            assert!(matches!(&out.output, FunctionCallOutput::Text(t) if t == "fn main() {}"));
        } else {
            panic!("Expected FunctionCallOutput, got {fco:?}");
        }
    }

    #[test]
    fn map_messages_handles_tool_errors() {
        let messages = vec![ChatMessage::ToolCallResult(Err(ToolCallError {
            id: "call_2".to_string(),
            name: "bash".to_string(),
            arguments: Some("{}".to_string()),
            error: "command failed".to_string(),
        }))];

        let (_, items) = map_messages(&messages).unwrap();
        assert_eq!(items.len(), 1);
        if let InputItem::Item(Item::FunctionCallOutput(out)) = &items[0] {
            assert!(matches!(&out.output, FunctionCallOutput::Text(t) if t.contains("Error: command failed")));
        } else {
            panic!("Expected FunctionCallOutput");
        }
    }

    #[test]
    fn map_messages_handles_summary() {
        let messages = vec![ChatMessage::Summary {
            content: "User asked about Rust.".to_string(),
            timestamp: IsoString::now(),
            messages_compacted: 5,
        }];

        let (_, items) = map_messages(&messages).unwrap();
        assert_eq!(items.len(), 1);
        if let InputItem::EasyMessage(msg) = &items[0] {
            assert_eq!(msg.role, Role::User);
            if let EasyInputContent::Text(text) = &msg.content {
                assert!(text.contains("Summary"));
                assert!(text.contains("Rust"));
            } else {
                panic!("Expected Text content");
            }
        } else {
            panic!("Expected EasyMessage");
        }
    }

    #[test]
    fn map_messages_serialization_shape() {
        let messages = vec![
            ChatMessage::user("Hello"),
            ChatMessage::Assistant {
                content: "Hi".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![ToolCallRequest {
                    id: "tc_1".to_string(),
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                }],
            },
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "tc_1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            })),
        ];

        let (_, items) = map_messages(&messages).unwrap();
        // EasyMessage items serialize with "type": "message"
        let json = serde_json::to_value(&items[0]).unwrap();
        assert_eq!(json["role"], "user");
        // FunctionCall items serialize with "type": "function_call"
        let json = serde_json::to_value(&items[2]).unwrap();
        assert_eq!(json["type"], "function_call");
        assert_eq!(json["call_id"], "tc_1");
        // FunctionCallOutput items serialize with "type": "function_call_output"
        let json = serde_json::to_value(&items[3]).unwrap();
        assert_eq!(json["type"], "function_call_output");
        assert_eq!(json["call_id"], "tc_1");
    }

    #[test]
    fn map_tools_produces_function_type() {
        let tools = vec![ToolDefinition::new(
            "read_file",
            "Read a file from disk",
            serde_json::from_str(r#"{"type": "object", "properties": {"path": {"type": "string"}}}"#).unwrap(),
        )];

        let mapped = map_tools(&tools, None).unwrap();
        assert_eq!(mapped.len(), 1);
        if let Tool::Function(f) = &mapped[0] {
            assert_eq!(f.name, "read_file");
            assert_eq!(f.description.as_deref(), Some("Read a file from disk"));
            assert_eq!(f.parameters.as_ref().unwrap()["properties"]["path"]["type"], "string");
        } else {
            panic!("Expected Tool::Function");
        }
    }

    #[test]
    fn map_messages_includes_encrypted_reasoning_item() {
        let messages = vec![ChatMessage::Assistant {
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

        let (_, items) = map_messages(&messages).unwrap();
        // Should have: easy_message (text) + reasoning item = 2
        assert_eq!(items.len(), 2);

        let reasoning_item = &items[1];
        if let InputItem::Item(Item::Reasoning(r)) = reasoning_item {
            assert_eq!(r.encrypted_content.as_deref(), Some("encrypted-blob"));
        } else {
            panic!("Expected Item::Reasoning, got {reasoning_item:?}");
        }
    }

    #[test]
    fn map_messages_skips_reasoning_item_without_encrypted_content() {
        let messages = vec![ChatMessage::Assistant {
            content: "no encrypted".to_string(),
            reasoning: AssistantReasoning::from_parts("just a summary".to_string(), None),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = map_messages(&messages).unwrap();
        // Only the text message, no reasoning item
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], InputItem::EasyMessage(_)));
    }

    #[test]
    fn map_messages_with_audio_errors() {
        let messages = vec![ChatMessage::User {
            content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
            timestamp: IsoString::now(),
        }];

        assert!(matches!(map_messages(&messages), Err(LlmError::UnsupportedContent(_))));
    }
}
