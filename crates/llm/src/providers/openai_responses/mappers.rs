use async_openai::types::responses::{
    CreateResponse, EasyInputContent, EasyInputMessage, FunctionCallOutput, FunctionCallOutputItemParam, FunctionTool,
    FunctionToolCall, ImageDetail, IncludeEnum, InputContent, InputImageContent, InputItem, InputParam,
    InputTextContent, Item, MessageType, Reasoning, ReasoningItem, ReasoningItemContent, ReasoningSummary,
    ReasoningTextContent, ResponseTextParam, Role, TextResponseFormatConfiguration, Tool, Verbosity,
};

use schemars::Schema;

use crate::catalog::Provider;
use crate::providers::openai_compatible::PromptCacheKeySource;
use crate::tool_schema::normalize_for_xiaomi;
use crate::{ChatMessage, ContentBlock, Context, LlmError, LlmModel, ReasoningEffort, Result, ToolDefinition};

/// The per-provider decisions that shape an otherwise identical Responses request.
pub struct ResponsesRequestPolicy {
    /// Provider that owns the model — decides whose encrypted reasoning may be
    /// replayed from earlier turns.
    pub(crate) provider: Provider,
    /// Send `reasoning` even when no effort was requested.
    always_include_reasoning: bool,
    /// Effort applied when the caller did not request one.
    default_effort: Option<ReasoningEffort>,
    text_verbosity: Option<Verbosity>,
    /// `strict` sent on every function tool. The Responses API defaults this to
    /// `true`, which rejects tool schemas that omit `additionalProperties`, so
    /// `None` and `Some(false)` are not interchangeable.
    tool_strict: Option<bool>,
    tool_schema_transform: Option<fn(&mut Schema)>,
    reasoning_format: ReasoningFormat,
    prompt_cache_key: PromptCacheKeySource,
}

impl ResponsesRequestPolicy {
    pub const OPENAI: Self = Self {
        provider: Provider::Openai,
        always_include_reasoning: false,
        default_effort: None,
        text_verbosity: None,
        tool_strict: Some(false),
        tool_schema_transform: None,
        reasoning_format: ReasoningFormat::Encrypted,
        prompt_cache_key: PromptCacheKeySource::Prefix,
    };

    pub const XIAOMI: Self = Self {
        provider: Provider::Xiaomi,
        tool_schema_transform: Some(normalize_for_xiaomi),
        reasoning_format: ReasoningFormat::PlainText,
        prompt_cache_key: PromptCacheKeySource::Omit,
        ..Self::OPENAI
    };

    #[cfg(feature = "codex")]
    pub const CODEX: Self = Self {
        provider: Provider::Codex,
        always_include_reasoning: true,
        default_effort: Some(ReasoningEffort::Medium),
        text_verbosity: Some(Verbosity::Medium),
        tool_strict: None,
        ..Self::OPENAI
    };

    #[cfg(feature = "bedrock")]
    pub const MANTLE: Self =
        Self { provider: Provider::Bedrock, always_include_reasoning: true, tool_strict: None, ..Self::OPENAI };

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

pub(crate) fn build_typed_request(
    model: &str,
    context: &Context,
    policy: &ResponsesRequestPolicy,
) -> Result<CreateResponse> {
    let identity: Option<LlmModel> = format!("{}:{model}", policy.provider.parser_name()).parse().ok();
    crate::provider::validate_reasoning(context, identity.as_ref())?;
    let context = context.filter_encrypted_reasoning(identity.as_ref());
    let encrypted = policy.reasoning_format == ReasoningFormat::Encrypted;
    let (instructions, input) = map_messages(context.messages(), policy.reasoning_format)?;
    let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), policy)?) };
    let settings = context.model_settings();
    let reasoning = (policy.always_include_reasoning || policy.effort(&context).is_some()).then_some(Reasoning {
        effort: None,
        summary: (encrypted && context.reasoning_effort() != ReasoningEffort::Disabled)
            .then_some(ReasoningSummary::Auto),
        mode: None,
        context: None,
    });

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
        include: encrypted.then_some(vec![IncludeEnum::ReasoningEncryptedContent]),
        text,
        prompt_cache_key: policy.prompt_cache_key.resolve(&context).map(String::from),
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
        let wire = match effort {
            ReasoningEffort::Default => return Ok(body),
            ReasoningEffort::Disabled if policy.provider == Provider::Codex => "disabled",
            ReasoningEffort::Disabled => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Xhigh => "xhigh",
            ReasoningEffort::Max => "max",
        };
        body["reasoning"]["effort"] = wire.into();
    }
    Ok(body)
}

pub(crate) fn map_user_content_for_responses(parts: &[ContentBlock]) -> Result<EasyInputContent> {
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentBlock::Text { text } => {
                items.push(InputContent::InputText(InputTextContent {
                    text: text.clone(),
                    prompt_cache_breakpoint: None,
                }));
            }
            ContentBlock::Image { .. } => {
                items.push(InputContent::InputImage(InputImageContent {
                    detail: ImageDetail::Auto,
                    file_id: None,
                    image_url: Some(part.as_data_uri().expect("image content always has a data URI")),
                    prompt_cache_breakpoint: None,
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
                if reasoning_format == ReasoningFormat::PlainText
                    && let Some(text) = &reasoning.summary_text
                {
                    items.push(InputItem::Item(Item::Reasoning(ReasoningItem {
                        id: None,
                        summary: vec![],
                        encrypted_content: None,
                        content: Some(vec![ReasoningItemContent::ReasoningText(ReasoningTextContent {
                            text: text.clone(),
                        })]),
                        status: None,
                    })));
                }
                if reasoning_format == ReasoningFormat::Encrypted
                    && let Some(encrypted) = &reasoning.encrypted_content
                {
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
                        caller: None,
                        r#async: None,
                    })));
                }
            }
            ChatMessage::ToolCallResult(result) => match result {
                Ok(r) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: Some(r.id.clone()),
                        output: FunctionCallOutput::Text(r.result.clone()),
                        id: None,
                        status: None,
                        name: None,
                        namespace: None,
                        caller: None,
                    })));
                }
                Err(e) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: Some(e.id.clone()),
                        output: FunctionCallOutput::Text(format!("Error: {}", e.error)),
                        id: None,
                        status: None,
                        name: None,
                        namespace: None,
                        caller: None,
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
fn map_tools(tools: &[ToolDefinition], policy: &ResponsesRequestPolicy) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            let parameters = match policy.tool_schema_transform {
                Some(transform) => {
                    let mut schema = Schema::try_from(tool.parameters.clone()).map_err(|error| {
                        LlmError::ToolParameterParsing { tool_name: tool.name.clone(), error: error.to_string() }
                    })?;
                    transform(&mut schema);
                    schema.into()
                }
                None => tool.parameters.clone(),
            };
            Ok(Tool::Function(FunctionTool {
                name: tool.name.clone(),
                description: Some(tool.description.clone()),
                parameters: Some(parameters),
                strict: policy.tool_strict,
                defer_loading: None,
                r#async: None,
                output_schema: None,
                allowed_callers: None,
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
        AssistantReasoning, ContentBlock, EncryptedReasoningContent, LlmError, MessageId, ToolCallError,
        ToolCallRequest, ToolCallResult,
    };

    fn openai_request(model: &str, context: &Context) -> Result<CreateResponse> {
        build_typed_request(model, context, &ResponsesRequestPolicy::OPENAI)
    }

    fn openai_body(model: &str, context: &Context) -> serde_json::Value {
        build_wire_request(model, context, &ResponsesRequestPolicy::OPENAI).unwrap()
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

        let body = build_wire_request("gpt-5.5", &context, &ResponsesRequestPolicy::CODEX).unwrap();

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

        let (system, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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

        let (system, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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
            assert_eq!(out.call_id.as_deref(), Some("call_1"));
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

        let (_, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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
            message_id: MessageId::new(),
            content: "User asked about Rust.".to_string(),
            timestamp: IsoString::now(),
            messages_compacted: 5,
        }];

        let (_, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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
                message_id: MessageId::new(),
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

        let (_, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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

        let mapped = map_tools(&tools, &ResponsesRequestPolicy::OPENAI).unwrap();
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

        let (_, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
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
            message_id: MessageId::new(),
            content: "no encrypted".to_string(),
            reasoning: AssistantReasoning::from_parts("just a summary".to_string(), None),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = map_messages(&messages, ReasoningFormat::Encrypted).unwrap();
        // Only the text message, no reasoning item
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], InputItem::EasyMessage(_)));
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
