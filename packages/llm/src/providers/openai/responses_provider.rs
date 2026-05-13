use std::collections::HashMap;

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::responses::{
    CreateResponse, EasyInputContent, EasyInputMessage, FunctionCallOutput, FunctionCallOutputItemParam, FunctionTool,
    FunctionToolCall, ImageDetail, IncludeEnum, InputContent, InputImageContent, InputItem, InputParam,
    InputTextContent, Item, MessageType, OutputItem, Reasoning, ReasoningEffort as OaiReasoningEffort,
    ReasoningSummary, ResponseStreamEvent, ResponseUsage, Role, Tool,
};
use tokio_stream::StreamExt;
use tracing::{debug, error};

use crate::provider::get_context_window;
use crate::providers::openai_compatible::AetherOpenAiConfig;
use crate::{
    ChatMessage, ContentBlock, Context, LlmError, LlmModel, LlmResponse, LlmResponseStream, ProviderAuthMode,
    ProviderConnectionConfig, ProviderFactory, ReasoningEffort, Result, StopReason, StreamingModelProvider, TokenUsage,
    ToolDefinition,
};

impl From<ResponseUsage> for TokenUsage {
    fn from(usage: ResponseUsage) -> Self {
        TokenUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: Some(usage.input_tokens_details.cached_tokens),
            reasoning_tokens: Some(usage.output_tokens_details.reasoning_tokens),
            ..TokenUsage::default()
        }
    }
}

pub(crate) fn map_user_content_for_responses(parts: &[ContentBlock]) -> Result<EasyInputContent> {
    let mut items = Vec::with_capacity(parts.len());
    for p in parts {
        match p {
            ContentBlock::Text { text } => {
                items.push(InputContent::InputText(InputTextContent { text: text.clone() }));
            }
            ContentBlock::Image { .. } => {
                items.push(InputContent::InputImage(InputImageContent {
                    detail: ImageDetail::Auto,
                    file_id: None,
                    image_url: Some(p.as_data_uri().unwrap()),
                }));
            }
            ContentBlock::Audio { .. } => {
                return Err(LlmError::UnsupportedContent("OpenAI Responses does not support audio input".into()));
            }
        }
    }
    Ok(EasyInputContent::ContentList(items))
}

pub struct OpenAiProvider {
    client: Client<AetherOpenAiConfig>,
    model: String,
}

impl ProviderFactory for OpenAiProvider {
    async fn from_env() -> Result<Self> {
        Self::from_env_with_connection(ProviderConnectionConfig::default()).await
    }

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        let api_key = match connection.auth_mode {
            ProviderAuthMode::Default => {
                std::env::var("OPENAI_API_KEY").map_err(|_| LlmError::MissingApiKey("OPENAI_API_KEY".to_string()))?
            }
            ProviderAuthMode::None => String::new(),
        };

        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Some(base_url) = connection.base_url {
            config = config.with_api_base(base_url);
        }
        let config = AetherOpenAiConfig::new(config, connection.auth_mode);

        Ok(Self { client: Client::with_config(config), model: "gpt-4.1".to_string() })
    }

    fn with_model(mut self, model: &str) -> Self {
        if !model.is_empty() {
            self.model = model.to_string();
        }
        self
    }
}

impl StreamingModelProvider for OpenAiProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let client = self.client.clone();
        let model = self.model.clone();
        let request = match build_response_request(&model, context) {
            Ok(req) => req,
            Err(e) => return Box::pin(async_stream::stream! { yield Err(e); }),
        };

        Box::pin(async_stream::stream! {
            debug!("Starting OpenAI Responses API stream for model: {model}");

            let stream = match client.responses().create_stream(request).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create OpenAI Responses stream: {e:?}");
                    yield Err(LlmError::ApiRequest(e.to_string()));
                    return;
                }
            };

            let mut stream = Box::pin(stream);
            let mut fn_calls: HashMap<String, (String, String)> = HashMap::new();
            let mut started = false;

            while let Some(result) = stream.next().await {
                match result {
                    Ok(event) => {
                        for response in process_event(event, &mut fn_calls, &mut started) {
                            yield response;
                        }
                    }
                    Err(e) => {
                        yield Err(LlmError::ApiError(e.to_string()));
                        break;
                    }
                }
            }

            if !started {
                yield Ok(LlmResponse::done());
            }
        })
    }

    fn display_name(&self) -> String {
        format!("OpenAI ({})", self.model)
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("openai", &self.model)
    }

    fn model(&self) -> Option<LlmModel> {
        format!("openai:{}", self.model).parse().ok()
    }
}

fn process_event(
    event: ResponseStreamEvent,
    fn_calls: &mut HashMap<String, (String, String)>,
    started: &mut bool,
) -> Vec<Result<LlmResponse>> {
    match event {
        ResponseStreamEvent::ResponseCreated(e) => {
            *started = true;
            vec![Ok(LlmResponse::start(&e.response.id))]
        }
        ResponseStreamEvent::ResponseOutputTextDelta(e) if !e.delta.is_empty() => {
            vec![Ok(LlmResponse::text(&e.delta))]
        }
        ResponseStreamEvent::ResponseReasoningSummaryTextDelta(e) if !e.delta.is_empty() => {
            vec![Ok(LlmResponse::reasoning(&e.delta))]
        }
        ResponseStreamEvent::ResponseOutputItemAdded(e) => {
            if let OutputItem::FunctionCall(fc) = e.item {
                let item_id = fc.id.clone().unwrap_or_default();
                fn_calls.insert(item_id, (fc.call_id.clone(), fc.name.clone()));
                vec![Ok(LlmResponse::tool_request_start(&fc.call_id, &fc.name))]
            } else {
                vec![]
            }
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(e) => {
            if let Some((call_id, _)) = fn_calls.get(&e.item_id) {
                vec![Ok(LlmResponse::tool_request_arg(call_id, &e.delta))]
            } else {
                vec![]
            }
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDone(e) => {
            if let Some((call_id, name)) = fn_calls.remove(&e.item_id) {
                let name = e.name.unwrap_or(name);
                vec![Ok(LlmResponse::tool_request_complete(&call_id, &name, &e.arguments))]
            } else {
                vec![]
            }
        }
        ResponseStreamEvent::ResponseCompleted(e) => {
            let mut results = Vec::new();
            if let Some(usage) = e.response.usage {
                results.push(Ok(LlmResponse::Usage { tokens: usage.into() }));
            }
            results.push(Ok(LlmResponse::done_with_stop_reason(StopReason::EndTurn)));
            results
        }
        ResponseStreamEvent::ResponseFailed(e) => {
            let msg = e.response.error.map_or_else(|| "Unknown error".to_string(), |err| err.message);
            vec![Err(LlmError::ApiError(msg))]
        }
        ResponseStreamEvent::ResponseIncomplete(_) => {
            vec![Ok(LlmResponse::done_with_stop_reason(StopReason::Length))]
        }
        ResponseStreamEvent::ResponseError(e) => {
            vec![Err(LlmError::ApiError(e.message))]
        }
        _ => vec![],
    }
}

fn build_response_request(model: &str, context: &Context) -> Result<CreateResponse> {
    let mut instructions: Option<String> = None;
    let mut items: Vec<InputItem> = Vec::new();

    for msg in context.messages() {
        match msg {
            ChatMessage::System { content, .. } => {
                instructions = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                items.push(InputItem::EasyMessage(EasyInputMessage {
                    r#type: MessageType::Message,
                    role: Role::User,
                    content: map_user_content_for_responses(content)?,
                    phase: None,
                }));
            }
            ChatMessage::Assistant { content, tool_calls, .. } => {
                if !content.is_empty() {
                    items.push(InputItem::EasyMessage(EasyInputMessage {
                        r#type: MessageType::Message,
                        role: Role::Assistant,
                        content: EasyInputContent::Text(content.clone()),
                        phase: None,
                    }));
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
            ChatMessage::ToolCallResult(result) => {
                let (call_id, output) = match result {
                    Ok(r) => (r.id.clone(), r.result.clone()),
                    Err(e) => (e.id.clone(), e.error.clone()),
                };
                items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                    call_id,
                    output: FunctionCallOutput::Text(output),
                    id: None,
                    status: None,
                })));
            }
            ChatMessage::Summary { content, .. } => {
                items.push(InputItem::EasyMessage(EasyInputMessage {
                    r#type: MessageType::Message,
                    role: Role::User,
                    content: EasyInputContent::Text(format!("[Previous conversation handoff]\n\n{content}")),
                    phase: None,
                }));
            }
            ChatMessage::Error { .. } => {}
        }
    }

    let tools = map_tools(context.tools())?;

    let reasoning = context
        .reasoning_effort()
        .map(|effort| Reasoning { effort: Some(map_reasoning_effort(effort)), summary: Some(ReasoningSummary::Auto) });

    Ok(CreateResponse {
        model: Some(model.to_string()),
        input: InputParam::Items(items),
        instructions,
        tools: if tools.is_empty() { None } else { Some(tools) },
        reasoning,
        stream: Some(true),
        include: Some(vec![IncludeEnum::ReasoningEncryptedContent]),
        store: Some(false),
        background: None,
        conversation: None,
        max_output_tokens: None,
        metadata: None,
        parallel_tool_calls: None,
        previous_response_id: None,
        prompt: None,
        service_tier: None,
        stream_options: None,
        temperature: None,
        text: None,
        tool_choice: None,
        top_p: None,
        truncation: None,
        prompt_cache_key: None,
        safety_identifier: None,
        max_tool_calls: None,
        prompt_cache_retention: None,
        top_logprobs: None,
    })
}

fn map_tools(tools: &[ToolDefinition]) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|t| {
            let parameters: serde_json::Value = serde_json::from_str(&t.parameters)
                .map_err(|e| LlmError::ToolParameterParsing { tool_name: t.name.clone(), error: e.to_string() })?;

            Ok(Tool::Function(FunctionTool {
                name: t.name.clone(),
                description: Some(t.description.clone()),
                parameters: Some(parameters),
                strict: Some(false),
                defer_loading: None,
            }))
        })
        .collect()
}

fn map_reasoning_effort(effort: ReasoningEffort) -> OaiReasoningEffort {
    match effort {
        ReasoningEffort::Low => OaiReasoningEffort::Low,
        ReasoningEffort::Medium => OaiReasoningEffort::Medium,
        ReasoningEffort::High => OaiReasoningEffort::High,
        ReasoningEffort::Xhigh => OaiReasoningEffort::Xhigh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssistantReasoning;
    use crate::ToolCallRequest;
    use crate::types::IsoString;

    #[test]
    fn test_build_request_simple_user_message() {
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );

        let req = build_response_request("gpt-4.1", &context).unwrap();
        assert_eq!(req.model, Some("gpt-4.1".to_string()));
        assert!(req.instructions.is_none());
        assert!(req.tools.is_none());
        assert!(req.reasoning.is_none());

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn test_build_request_with_system_message() {
        let context = Context::new(
            vec![
                ChatMessage::System { content: "You are helpful.".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() },
            ],
            vec![],
        );

        let req = build_response_request("gpt-4.1", &context).unwrap();
        assert_eq!(req.instructions, Some("You are helpful.".to_string()));

        let json = serde_json::to_value(&req).unwrap();
        let items = json["input"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
    }

    #[test]
    fn test_build_request_with_tool_calls() {
        let context = Context::new(
            vec![
                ChatMessage::User { content: vec![ContentBlock::text("Search for rust")], timestamp: IsoString::now() },
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
                ChatMessage::ToolCallResult(Ok(crate::ToolCallResult {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"q":"rust"}"#.to_string(),
                    result: "Found results".to_string(),
                })),
            ],
            vec![ToolDefinition {
                name: "search".to_string(),
                description: "Search".to_string(),
                parameters: r#"{"type":"object"}"#.to_string(),
                server: None,
            }],
        );

        let req = build_response_request("gpt-4.1", &context).unwrap();
        let json = serde_json::to_value(&req).unwrap();

        let items = json["input"].as_array().unwrap();
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_1");
        assert_eq!(items[2]["output"], "Found results");

        assert!(req.tools.is_some());
        let tools_json = serde_json::to_value(&req.tools).unwrap();
        assert_eq!(tools_json[0]["type"], "function");
        assert_eq!(tools_json[0]["name"], "search");
    }

    #[test]
    fn test_build_request_with_reasoning_effort() {
        let mut context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Think")], timestamp: IsoString::now() }],
            vec![],
        );
        context.set_reasoning_effort(Some(ReasoningEffort::High));

        let req = build_response_request("o3", &context).unwrap();
        let reasoning = req.reasoning.unwrap();
        assert_eq!(reasoning.effort, Some(OaiReasoningEffort::High));
        assert_eq!(reasoning.summary, Some(ReasoningSummary::Auto));
    }

    #[test]
    fn test_build_request_with_audio_returns_unsupported_content() {
        let context = Context::new(
            vec![ChatMessage::User {
                content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        assert!(matches!(build_response_request("gpt-4.1", &context), Err(LlmError::UnsupportedContent(_))));
    }

    #[test]
    fn test_map_tools_valid() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: r#"{"type":"object","properties":{"path":{"type":"string"}}}"#.to_string(),
            server: None,
        }];

        let result = map_tools(&tools).unwrap();
        assert_eq!(result.len(), 1);

        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["name"], "read_file");
    }

    #[test]
    fn test_map_tools_invalid_json() {
        let tools = vec![ToolDefinition {
            name: "broken".to_string(),
            description: "Broken".to_string(),
            parameters: "not json{".to_string(),
            server: None,
        }];

        let result = map_tools(&tools);
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ToolParameterParsing { tool_name, .. } => {
                assert_eq!(tool_name, "broken");
            }
            other => panic!("Expected ToolParameterParsing, got: {other}"),
        }
    }

    #[test]
    fn test_provider_display_name() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("test"), ProviderAuthMode::Default);
        let provider = OpenAiProvider { client: Client::with_config(config), model: "gpt-4.1".to_string() };
        assert_eq!(provider.display_name(), "OpenAI (gpt-4.1)");
    }
}
