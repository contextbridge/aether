#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/generic.md"))]

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use std::future::Future;
use std::time::Duration;

use crate::catalog::Provider;
use crate::provider::{display_name, validate_reasoning};
use crate::providers::anthropic::mappers::build_request as build_messages_request;
use crate::providers::anthropic::streaming::decode_line;
use crate::providers::http::{SseData, anthropic_code, http_client, open_sse};
pub use crate::providers::openai_compatible::ChatRequestPolicy;
use crate::providers::openai_compatible::streaming::decode_sse_chunk;
use crate::providers::openai_compatible::types::CompatibleChatRequest;
use crate::providers::openai_compatible::{PromptCacheKeySource, build_chat_request};
use crate::providers::openai_responses::mappers::build_wire_request;
use crate::providers::openai_responses::streaming::decode_responses;
use crate::providers::openai_responses::transport::send;
use crate::providers::response_stream::{OpenedStream, error_stream, response_stream};
use crate::tool_schema::normalize_for_moonshot;
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, Result,
    StreamingModelProvider,
};

pub use crate::providers::openai_responses::mappers::ResponsesRequestPolicy;

pub struct ProviderConfig {
    pub provider: Provider,
    pub api_base: Option<&'static str>,
    pub default_model: &'static str,
    pub api: Api,
}

pub enum Api {
    ChatCompletions(ChatRequestPolicy),
    Responses(ResponsesRequestPolicy),
    Messages,
}

pub const ANTHROPIC: ProviderConfig = ProviderConfig {
    provider: Provider::Anthropic,
    api_base: Some("https://api.anthropic.com"),
    default_model: "claude-sonnet-4-5-20250929",
    api: Api::Messages,
};

pub const OPENAI: ProviderConfig = ProviderConfig {
    provider: Provider::Openai,
    api_base: Some("https://api.openai.com/v1"),
    default_model: "gpt-4.1",
    api: Api::Responses(ResponsesRequestPolicy::OPENAI),
};

pub const XIAOMI: ProviderConfig = ProviderConfig {
    provider: Provider::Xiaomi,
    api_base: Some("https://api.xiaomimimo.com/v1"),
    default_model: "mimo-v2.6-pro",
    api: Api::Responses(ResponsesRequestPolicy::XIAOMI),
};

pub const DEEPSEEK: ProviderConfig = ProviderConfig {
    provider: Provider::DeepSeek,
    api_base: Some("https://api.deepseek.com"),
    default_model: "deepseek-v4-flash",
    api: Api::ChatCompletions(ChatRequestPolicy::DEFAULT),
};

pub const MOONSHOT: ProviderConfig = ProviderConfig {
    provider: Provider::Moonshot,
    api_base: Some("https://api.moonshot.ai/v1"),
    default_model: "moonshot-v1-8k",
    api: Api::ChatCompletions(ChatRequestPolicy {
        tool_schema_transform: Some(normalize_for_moonshot),
        ..ChatRequestPolicy::DEFAULT
    }),
};

pub const ZAI: ProviderConfig = ProviderConfig {
    provider: Provider::ZAi,
    api_base: Some("https://api.z.ai/api/coding/paas/v4"),
    default_model: "GLM-4.6",
    api: Api::ChatCompletions(ChatRequestPolicy::DEFAULT),
};

pub const AZURE_FOUNDRY: ProviderConfig = ProviderConfig {
    provider: Provider::AzureFoundry,
    api_base: None,
    default_model: "gpt-5.5",
    api: Api::ChatCompletions(ChatRequestPolicy {
        prompt_cache_key: PromptCacheKeySource::Prefix,
        ..ChatRequestPolicy::DEFAULT
    }),
};

pub const FIREWORKS: ProviderConfig = ProviderConfig {
    provider: Provider::Fireworks,
    api_base: Some("https://api.fireworks.ai/inference/v1"),
    default_model: "accounts/fireworks/models/glm-5p1",
    api: Api::ChatCompletions(ChatRequestPolicy {
        prompt_cache_key: PromptCacheKeySource::SessionAffinity,
        ..ChatRequestPolicy::DEFAULT
    }),
};

pub const GEMINI: ProviderConfig = ProviderConfig {
    provider: Provider::Gemini,
    api_base: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
    default_model: "",
    api: Api::ChatCompletions(ChatRequestPolicy::DEFAULT),
};

pub const OPENROUTER: ProviderConfig = ProviderConfig {
    provider: Provider::OpenRouter,
    api_base: Some("https://openrouter.ai/api/v1"),
    default_model: "",
    api: Api::ChatCompletions(ChatRequestPolicy {
        prompt_cache_key: PromptCacheKeySource::Prefix,
        customize: Some(openrouter_request),
        ..ChatRequestPolicy::DEFAULT
    }),
};

pub const OLLAMA: ProviderConfig = ProviderConfig {
    provider: Provider::Ollama,
    api_base: Some("http://localhost:11434/v1"),
    default_model: "",
    api: Api::ChatCompletions(LOCAL_CHAT),
};

pub const LLAMACPP: ProviderConfig = ProviderConfig {
    provider: Provider::LlamaCpp,
    api_base: Some("http://localhost:8080/v1"),
    default_model: "",
    api: Api::ChatCompletions(LOCAL_CHAT),
};

const LOCAL_CHAT: ChatRequestPolicy =
    ChatRequestPolicy { tool_call_reasoning_placeholder: false, ..ChatRequestPolicy::DEFAULT };

pub(crate) const BUILT_INS: &[&ProviderConfig] = &[
    &ANTHROPIC,
    &OPENAI,
    &XIAOMI,
    &DEEPSEEK,
    &MOONSHOT,
    &ZAI,
    &AZURE_FOUNDRY,
    &FIREWORKS,
    &GEMINI,
    &OPENROUTER,
    &OLLAMA,
    &LLAMACPP,
];

/// A provider whose behavior is fully described by a [`ProviderConfig`].
pub struct GenericProvider {
    config: &'static ProviderConfig,
    http: reqwest::Client,
    api_base: String,
    headers: HeaderMap,
    model: String,
    request_model: Option<String>,
    idle_timeout: Duration,
}

impl GenericProvider {
    pub fn from_env(config: &'static ProviderConfig) -> Result<Self> {
        Self::from_env_with_connection(config, ProviderConnectionConfig::default())
    }

    pub fn from_env_with_connection(
        config: &'static ProviderConfig,
        connection: ProviderConnectionConfig,
    ) -> Result<Self> {
        let api_key = match (connection.auth_mode, config.provider.required_env_var()) {
            (ProviderAuthMode::Default, Some(env_var)) => {
                std::env::var(env_var).map_err(|_| LlmError::MissingApiKey(env_var.to_string()))?
            }
            _ => String::new(),
        };
        Self::new_with_connection(&api_key, config, connection)
    }

    pub fn new(api_key: &str, config: &'static ProviderConfig) -> Result<Self> {
        Self::new_with_connection(api_key, config, ProviderConnectionConfig::default())
    }

    pub fn new_with_connection(
        api_key: &str,
        config: &'static ProviderConfig,
        connection: ProviderConnectionConfig,
    ) -> Result<Self> {
        let api_base = connection
            .base_url
            .or_else(|| config.api_base.map(str::to_string))
            .ok_or_else(|| LlmError::MissingProviderUrl { provider: config.provider.parser_name().to_string() })?;
        let mut api_base = api_base.trim_end_matches('/').to_string();
        if config.provider.is_local() && !api_base.ends_with("/v1") {
            api_base.push_str("/v1");
        }
        Ok(Self {
            config,
            http: http_client(),
            api_base,
            headers: api_headers(&config.api, api_key, connection.auth_mode)?,
            model: config.default_model.to_string(),
            request_model: connection.request_model,
            idle_timeout: connection.idle_timeout,
        })
    }

    pub fn with_model(mut self, model: &str) -> Self {
        if !model.is_empty() {
            self.model = model.to_string();
        }
        self
    }
}

impl StreamingModelProvider for GenericProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let stream = match &self.config.api {
            Api::ChatCompletions(policy) => self.stream_chat_completions(context, policy),
            Api::Responses(policy) => self.stream_responses(context, policy),
            Api::Messages => self.stream_messages(context),
        };
        stream.unwrap_or_else(error_stream)
    }

    fn display_name(&self) -> String {
        display_name(self.config.provider, &self.model)
    }

    fn model(&self) -> Option<LlmModel> {
        format!("{}:{}", self.config.provider.parser_name(), self.model).parse().ok()
    }
}

impl GenericProvider {
    fn stream_chat_completions(&self, context: &Context, policy: &ChatRequestPolicy) -> Result<LlmResponseStream> {
        validate_reasoning(context, self.model().as_ref())?;
        let model = self.request_model.as_deref().unwrap_or(&self.model);
        let request = serde_json::to_value(build_chat_request(model, context, policy)?)?;
        let opened = self.open("chat/completions", request);
        Ok(response_stream(opened, |data, turn| decode_sse_chunk(&data, turn), self.idle_timeout))
    }

    fn stream_responses(&self, context: &Context, policy: &ResponsesRequestPolicy) -> Result<LlmResponseStream> {
        let mut request = build_wire_request(&self.model, context, policy)?;
        if let Some(model) = &self.request_model {
            request["model"] = model.clone().into();
        }
        Ok(response_stream(self.open("responses", request), decode_responses(), self.idle_timeout))
    }

    fn stream_messages(&self, context: &Context) -> Result<LlmResponseStream> {
        validate_reasoning(context, self.model().as_ref())?;
        let model = self.request_model.as_deref().unwrap_or(&self.model);
        let request = build_messages_request(model, context)?;
        let builder =
            self.http.post(format!("{}/v1/messages", self.api_base)).headers(self.headers.clone()).json(&request);
        Ok(response_stream(open_sse(builder, anthropic_code), |data, turn| decode_line(&data, turn), self.idle_timeout))
    }

    fn open(
        &self,
        path: &str,
        request: serde_json::Value,
    ) -> impl Future<Output = Result<OpenedStream<SseData>>> + Send + 'static {
        let (http, url, headers) = (self.http.clone(), format!("{}/{path}", self.api_base), self.headers.clone());
        async move { send(&http, &url, headers, request).await }
    }
}

fn api_headers(api: &Api, api_key: &str, auth_mode: ProviderAuthMode) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    if let Api::Messages = api {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    if auth_mode == ProviderAuthMode::Default && !api_key.is_empty() {
        let (name, value) = match api {
            Api::ChatCompletions(_) | Api::Responses(_) => (AUTHORIZATION, format!("Bearer {api_key}")),
            Api::Messages => (HeaderName::from_static("x-api-key"), api_key.to_string()),
        };
        let mut value = HeaderValue::from_str(&value)?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}

fn openrouter_request(request: &mut CompatibleChatRequest, context: &Context) {
    request.extra.insert("usage".into(), serde_json::json!({ "include": true }));
    request.extra.insert("cache_control".into(), serde_json::json!({ "type": "ephemeral" }));
    if let Some(session_id) = context.session_affinity_key() {
        request.extra.insert("session_id".into(), session_id.into());
    }
    if request.reasoning_effort == Some("none") {
        request.reasoning_effort = None;
        request.extra.insert("reasoning".into(), serde_json::json!({ "effort": "none" }));
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::providers::test_capture_server::{CaptureServer, ResponseSpec, hello_context};
    use crate::types::IsoString;
    use crate::{
        AssistantReasoning, ChatMessage, ContentBlock, LlmResponse, MessageId, ModelSettings, ProviderErrorKind,
        ReasoningEffort, ToolCallRequest, ToolDefinition,
    };

    #[tokio::test]
    async fn gemini_disables_reasoning_with_none() {
        let model = reasoning_off_model(Provider::Gemini);
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = capture_backed_provider(&server, &GEMINI).with_model(&model.model_id());
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::Disabled);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert_successful_stream(&responses);
        assert_eq!(server.captured().await.body["reasoning_effort"], "none");
    }

    #[tokio::test]
    async fn moonshot_normalizes_tool_schemas() {
        let parameters = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "MySchema",
            "type": "object",
            "properties": { "q": { "type": "string", "format": "uri" } }
        });
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = capture_backed_provider(&server, &MOONSHOT);
        let context =
            Context::new(vec![ChatMessage::user("hi")], vec![ToolDefinition::new("search", "Search", parameters)]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = server.captured().await.body;

        assert_successful_stream(&responses);
        let params = &body["tools"][0]["function"]["parameters"];
        assert!(params.get("$schema").is_none());
        assert!(params.get("title").is_none());
        assert!(params["properties"]["q"].get("format").is_none());
        assert_eq!(params["type"], "object");
    }

    #[tokio::test]
    async fn chat_completions_send_errors_and_summaries_as_user_notes() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = capture_backed_provider(&server, &DEEPSEEK);
        let context = Context::new(
            vec![
                ChatMessage::Error {
                    message_id: MessageId::new(),
                    message: "boom".to_string(),
                    timestamp: IsoString::now(),
                },
                ChatMessage::Summary {
                    message_id: MessageId::new(),
                    content: "earlier work".to_string(),
                    timestamp: IsoString::now(),
                    messages_compacted: 2,
                },
            ],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = server.captured().await.body;

        assert_successful_stream(&responses);
        assert_eq!(body["messages"][0], json!({ "role": "user", "content": "Error: boom" }));
        assert_eq!(
            body["messages"][1],
            json!({ "role": "user", "content": "[Previous conversation handoff]\n\nearlier work" })
        );
    }

    #[tokio::test]
    async fn openrouter_requests_usage_caching_and_session_routing() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = capture_backed_provider(&server, &OPENROUTER).with_model("anthropic/claude-haiku-4.5");
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_prompt_cache_key(Some("prefix-abc".to_string()));
        context.set_session_affinity_key(Some("conversation-abc".to_string()));
        context.set_reasoning_effort(ReasoningEffort::High);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = server.captured().await.body;

        assert_successful_stream(&responses);
        assert_eq!(body["model"], "anthropic/claude-haiku-4.5");
        assert_eq!(body["prompt_cache_key"], "prefix-abc");
        assert_eq!(body["session_id"], "conversation-abc");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["cache_control"]["type"], "ephemeral");
        assert_eq!(body["usage"]["include"], true);
        assert_eq!(provider.display_name(), "OpenRouter (anthropic/claude-haiku-4.5)");
    }

    #[tokio::test]
    async fn openrouter_disables_reasoning_through_its_unified_reasoning_object() {
        let model = reasoning_off_model(Provider::OpenRouter);
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = capture_backed_provider(&server, &OPENROUTER).with_model(&model.model_id());
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::Disabled);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = server.captured().await.body;

        assert_successful_stream(&responses);
        assert_eq!(body["reasoning"], json!({ "effort": "none" }));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn chat_rate_limit_rejection_is_a_retryable_provider_error() {
        let body = r#"{"error":{"message":"Rate limit exceeded: free-models-per-day","code":429}}"#;
        let error = rejected_chat_request(ResponseSpec::sse(body).with_status(429)).await;
        let provider_error = error.provider().expect("a 429 must classify as a provider error");

        assert_eq!(provider_error.kind, ProviderErrorKind::RateLimit);
        assert_eq!(provider_error.http_status, Some(429));
        assert_eq!(provider_error.code.as_deref(), Some("429"));
        assert_eq!(provider_error.request_id.as_deref(), Some("request-123"));
        assert!(provider_error.message.contains("free-models-per-day"));
        assert!(error.is_retryable(), "429 must be retryable, got {error}");
    }

    #[tokio::test]
    async fn chat_rejection_preserves_string_error_codes() {
        let body = r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#;
        let error = rejected_chat_request(ResponseSpec::sse(body).with_status(429)).await;
        let provider_error = error.provider().unwrap();

        assert_eq!(provider_error.code.as_deref(), Some("rate_limit_exceeded"));
        assert!(provider_error.message.contains("slow down"));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn chat_bad_request_is_not_retryable() {
        let body = r#"{"error":{"message":"invalid request"}}"#;
        let error = rejected_chat_request(ResponseSpec::sse(body).with_status(400)).await;

        assert_eq!(error.provider().map(|provider| provider.kind), Some(ProviderErrorKind::Api));
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn chat_rejection_keeps_http_status_when_the_error_body_fails_to_read() {
        let error = rejected_chat_request(ResponseSpec::sse("").with_status(429).interrupted()).await;
        let provider_error = error.provider().unwrap();

        assert!(provider_error.message.ends_with("Unknown error"), "{provider_error:?}");
        assert_eq!(provider_error.http_status, Some(429));
        assert_eq!(provider_error.request_id.as_deref(), Some("request-123"));
        assert_eq!(provider_error.kind, ProviderErrorKind::RateLimit);
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn chat_chunk_that_is_not_json_interrupts_the_stream() {
        let responses = streamed_chat("data: invalid json\n\n").await;

        assert!(matches!(responses[..], [Ok(LlmResponse::Start), Err(_)]), "{responses:?}");
        assert_interrupted(&responses);
    }

    #[tokio::test]
    async fn chat_stream_ends_at_done() {
        let responses = streamed_chat(&format!("{CHAT_FIXTURE}data: invalid json\n\n")).await;

        assert_successful_stream(&responses);
    }

    #[tokio::test]
    async fn chat_stream_skips_keepalive_events() {
        let responses = streamed_chat(&format!("event: keepalive\ndata: ping\n\n{CHAT_FIXTURE}")).await;

        assert_successful_stream(&responses);
    }

    #[tokio::test]
    async fn chat_stream_cut_off_mid_tool_call_is_interrupted() {
        let tool_call_start = json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "bash", "arguments": r#"{"command":"# }
        }] } }] });

        let responses = streamed_chat(&format!("data: {tool_call_start}\n\n")).await;

        assert!(
            !responses.iter().any(|response| matches!(
                response,
                Ok(LlmResponse::ToolRequestComplete { .. } | LlmResponse::Done { .. })
            )),
            "{responses:?}"
        );
        assert_interrupted(&responses);
    }

    #[tokio::test]
    async fn disabled_toggle_and_unknown_models_never_send_requests() {
        let server = CaptureServer::start_chat_completions().await;
        let mut provider = capture_backed_provider(&server, &DEEPSEEK);
        for model in ["deepseek-v4-flash", "unknown"] {
            provider = provider.with_model(model);
            let mut context = Context::new(vec![], vec![]);
            context.set_reasoning_effort(ReasoningEffort::Disabled);
            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            assert_eq!(responses.len(), 1, "{responses:?}");
            let error = responses[0].as_ref().unwrap_err();
            if model == "unknown" {
                assert!(matches!(error, LlmError::ReasoningValidation(_)));
            } else {
                assert!(matches!(error, LlmError::UnsupportedDisableTransport { .. }));
            }
            assert!(!error.is_retryable());
        }
    }

    #[test]
    fn display_name_names_models_outside_the_catalog() {
        let provider = GenericProvider::new("key", &DEEPSEEK).unwrap().with_model("my-custom-model");
        assert_eq!(provider.display_name(), "DeepSeek (my-custom-model)");

        let provider = GenericProvider::new("key", &GEMINI).unwrap();
        assert_eq!(provider.display_name(), "Gemini");
    }

    #[test]
    fn azure_foundry_requires_a_configured_url() {
        let Err(error) = GenericProvider::new("key", &AZURE_FOUNDRY) else {
            panic!("Azure Foundry must require a URL");
        };
        assert!(matches!(error, LlmError::MissingProviderUrl { provider } if provider == "azure-foundry"));
    }

    #[tokio::test]
    async fn chat_request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = GenericProvider::new_with_connection(
            "key",
            &AZURE_FOUNDRY,
            ProviderConnectionConfig {
                base_url: Some(format!("{}/", server.base_url)),
                auth_mode: ProviderAuthMode::None,
                request_model: Some("production-coding".to_string()),
                ..Default::default()
            },
        )
        .unwrap()
        .with_model("gpt-5.5");
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert_successful_stream(&responses);
        assert_eq!(captured.path, "/chat/completions");
        assert_eq!(captured.body["model"], "production-coding");
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.body["stream_options"]["include_usage"], true);
        assert!(captured.headers.get("authorization").is_none());
        assert_eq!(provider.model().unwrap().to_string(), "azure-foundry:gpt-5.5");
        assert_eq!(provider.display_name(), "Microsoft Foundry (gpt-5.5)");
    }

    #[tokio::test]
    async fn default_auth_sends_the_api_key_as_a_bearer_token() {
        let mut server = CaptureServer::start_chat_completions().await;
        let connection = ProviderConnectionConfig { base_url: Some(server.base_url.clone()), ..Default::default() };
        let provider = GenericProvider::new_with_connection("secret", &DEEPSEEK, connection).unwrap();

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;

        assert_successful_stream(&responses);
        assert_eq!(server.captured().await.headers["authorization"], "Bearer secret");
    }

    #[tokio::test]
    async fn chat_providers_apply_their_declared_prompt_cache_policy() {
        for (config, expected_key) in [
            (&AZURE_FOUNDRY, Some("prefix-abc")),
            (&FIREWORKS, Some("conversation-abc")),
            (&DEEPSEEK, None),
            (&MOONSHOT, None),
            (&ZAI, None),
        ] {
            let mut server = CaptureServer::start_chat_completions().await;
            let provider = capture_backed_provider(&server, config);
            let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
            context.set_prompt_cache_key(Some("prefix-abc".to_string()));
            context.set_session_affinity_key(Some("conversation-abc".to_string()));

            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            let captured = server.captured().await;

            assert_successful_stream(&responses);
            assert_eq!(captured.body.get("prompt_cache_key").and_then(serde_json::Value::as_str), expected_key);
            assert!(captured.body.get("user").is_none());
            assert!(captured.body.get("session_id").is_none());
        }
    }

    #[tokio::test]
    async fn chat_providers_omit_unset_context_keys() {
        for config in [&AZURE_FOUNDRY, &FIREWORKS] {
            let mut server = CaptureServer::start_chat_completions().await;
            let provider = capture_backed_provider(&server, config);
            let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            let captured = server.captured().await;

            assert_successful_stream(&responses);
            assert!(captured.body.get("prompt_cache_key").is_none());
            assert!(captured.body.get("session_id").is_none());
        }
    }

    #[tokio::test]
    async fn local_providers_send_bare_chat_requests_under_v1() {
        for (config, display_name) in [(&OLLAMA, "Ollama (test-model)"), (&LLAMACPP, "LlamaCpp (test-model)")] {
            let mut server = CaptureServer::start_chat_completions().await;
            let connection = ProviderConnectionConfig { base_url: Some(server.base_url.clone()), ..Default::default() };
            let provider =
                GenericProvider::from_env_with_connection(config, connection).unwrap().with_model("test-model");
            let tool_call = ChatMessage::Assistant {
                message_id: MessageId::new(),
                content: String::new(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![ToolCallRequest { id: "call_1".into(), name: "tool".into(), arguments: "{}".into() }],
            };
            let mut context = Context::new(vec![ChatMessage::user("Hello"), tool_call], vec![]);
            context.set_prompt_cache_key(Some("prefix-abc".to_string()));
            context.set_session_affinity_key(Some("conversation-abc".to_string()));

            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            let captured = server.captured().await;

            assert_successful_stream(&responses);
            assert_eq!(captured.path, "/v1/chat/completions");
            assert_eq!(captured.body["model"], "test-model");
            assert!(captured.headers.get("authorization").is_none());
            assert!(captured.body.get("prompt_cache_key").is_none());
            assert!(captured.body["messages"][1].get("reasoning_content").is_none());
            assert_eq!(provider.display_name(), display_name);
        }
    }

    #[tokio::test]
    async fn providers_honor_configured_idle_timeout() {
        for (config, fixture) in [
            (&OLLAMA, include_str!("../../tests/fixtures/openai/01_minimal.sse")),
            (&ANTHROPIC, include_str!("../../tests/fixtures/anthropic/01_minimal.sse")),
        ] {
            let spec = ResponseSpec::sse(fixture).paced(Duration::from_mins(2));
            let mut server = CaptureServer::start_with_spec(spec).await;
            let connection = ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                auth_mode: ProviderAuthMode::None,
                idle_timeout: Duration::from_mins(1),
                ..Default::default()
            };
            let provider = GenericProvider::new_with_connection("key", config, connection).unwrap();

            let responses = server.collect_on_paused_clock(provider.stream_response(&hello_context())).await;

            let error = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
            assert_eq!(error.map(|error| error.kind), Some(ProviderErrorKind::Timeout), "{responses:?}");
        }
    }

    #[tokio::test]
    async fn openai_distinguishes_default_disabled_and_low_effort() {
        for (effort, expected) in [
            (ReasoningEffort::Default, None),
            (ReasoningEffort::Disabled, Some("none")),
            (ReasoningEffort::Low, Some("low")),
        ] {
            let mut server = CaptureServer::start_responses().await;
            let provider = capture_backed_provider(&server, &OPENAI).with_model("gpt-5.4");
            let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
            context.set_reasoning_effort(effort);

            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            let body = server.captured().await.body;

            assert!(responses.iter().all(Result::is_ok), "{responses:?}");
            assert_eq!(body["reasoning"]["effort"].as_str(), expected);
            if effort == ReasoningEffort::Disabled {
                assert!(body["reasoning"]["summary"].is_null());
            }
        }
    }

    #[tokio::test]
    async fn openai_sends_max_effort_and_prompt_cache_key() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &OPENAI).with_model("gpt-5.6");
        let mut context = Context::new(vec![ChatMessage::user("Think harder")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::Max);
        context.set_prompt_cache_key(Some("cache-key".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert_eq!(captured.body["model"], "gpt-5.6");
        assert_eq!(captured.body["prompt_cache_key"], "cache-key");
        assert_eq!(captured.body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(captured.body["stream"], true);
        assert_eq!(provider.display_name(), "OpenAI (gpt-5.6)");
    }

    #[tokio::test]
    async fn responses_http_200_failed_server_error_is_retryable_with_request_id() {
        let spec = ResponseSpec::sse(include_str!("../../tests/fixtures/openai_responses/04_failed_server.sse"))
            .with_header("x-request-id", "req-openai-1");
        let mut server = CaptureServer::start_with_spec(spec).await;
        let provider = capture_backed_provider(&server, &OPENAI);
        let context = Context::new(vec![ChatMessage::user("hi")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(err.is_retryable(), "server_error must be retryable: {err:?}");
        let provider_error = err.provider().expect("expected provider error");
        assert_eq!(provider_error.kind, ProviderErrorKind::Server);
        assert_eq!(provider_error.http_status, Some(200));
        assert_eq!(provider_error.request_id.as_deref(), Some("req-openai-1"));
        assert_eq!(provider_error.code.as_deref(), Some("server_error"));
    }

    #[tokio::test]
    async fn responses_surface_a_mapping_failure_as_the_only_item() {
        let connection = ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() };
        let provider = GenericProvider::from_env_with_connection(&OPENAI, connection).unwrap();
        let context = Context::new(
            vec![ChatMessage::User {
                message_id: MessageId::new(),
                content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], Err(LlmError::UnsupportedContent(_))), "{responses:?}");
    }

    #[tokio::test]
    async fn xiaomi_omits_encrypted_reasoning_summaries_and_prompt_cache_key() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::High);
        context.set_prompt_cache_key(Some("cache-key".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.path, "/responses");
        assert_eq!(captured.body["model"], "mimo-v2.6-pro");
        assert_eq!(captured.body["reasoning"], json!({ "effort": "high" }));
        assert!(captured.body.get("include").is_none());
        assert!(captured.body.get("prompt_cache_key").is_none());
        assert_eq!(provider.display_name(), "Xiaomi (mimo-v2.6-pro)");
    }

    #[tokio::test]
    async fn xiaomi_replays_prior_reasoning_as_plain_text() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let context = Context::new(
            vec![
                ChatMessage::user("Hello"),
                ChatMessage::Assistant {
                    message_id: MessageId::new(),
                    content: "Hi".to_string(),
                    reasoning: AssistantReasoning::from_parts("greeting the user".to_string(), None),
                    timestamp: IsoString::now(),
                    tool_calls: vec![],
                },
                ChatMessage::user("Again"),
            ],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let reasoning = captured.body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "reasoning")
            .expect("reasoning item should be replayed");
        assert_eq!(reasoning["content"], json!([{ "type": "reasoning_text", "text": "greeting the user" }]));
        assert!(reasoning.get("encrypted_content").is_none_or(serde_json::Value::is_null));
    }

    #[tokio::test]
    async fn xiaomi_drops_null_from_optional_tool_parameters() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let tool = ToolDefinition::new(
            "bash",
            "Run a command",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "description": { "type": ["string", "null"] }
                },
                "required": ["command"]
            }),
        );
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![tool]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["tools"][0]["parameters"]["properties"]["description"], json!({ "type": "string" }));
    }

    #[tokio::test]
    async fn responses_request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_responses().await;
        let provider = GenericProvider::new_with_connection(
            "key",
            &XIAOMI,
            ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                auth_mode: ProviderAuthMode::None,
                request_model: Some("mimo-deployment".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["model"], "mimo-deployment");
        assert_eq!(provider.model().unwrap().to_string(), "xiaomi:mimo-v2.6-pro");
    }

    #[tokio::test]
    async fn anthropic_sends_its_version_and_api_key_unless_auth_is_none() {
        for (auth_mode, api_key) in [(ProviderAuthMode::Default, Some("secret")), (ProviderAuthMode::None, None)] {
            let mut server = CaptureServer::start_messages().await;
            let connection =
                ProviderConnectionConfig { base_url: Some(server.base_url.clone()), auth_mode, ..Default::default() };
            let provider = GenericProvider::new_with_connection("secret", &ANTHROPIC, connection).unwrap();

            let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
            let captured = server.captured().await;

            assert_successful_stream(&responses);
            assert_eq!(captured.path, "/v1/messages");
            assert_eq!(captured.headers["anthropic-version"], "2023-06-01");
            assert_eq!(captured.headers.get("x-api-key").map(|key| key.to_str().unwrap()), api_key);
            assert!(captured.headers.get("authorization").is_none());
        }
    }

    #[tokio::test]
    async fn anthropic_caches_the_system_prompt_and_sends_tools() {
        let mut server = CaptureServer::start_messages().await;
        let provider = capture_backed_provider(&server, &ANTHROPIC);
        let context = Context::new(
            vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")],
            vec![ToolDefinition::new("search", "Search", json!({ "type": "object" }))],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = server.captured().await.body;

        assert_successful_stream(&responses);
        assert_eq!(body["model"], "claude-sonnet-4-5-20250929");
        assert_eq!(body["max_tokens"], 16_384);
        assert_eq!(body["stream"], true);
        assert_eq!(body["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "You are helpful", "cache_control": { "type": "ephemeral" } }])
        );
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body["tools"],
            json!([{ "name": "search", "description": "Search", "input_schema": { "type": "object" } }])
        );
        assert_eq!(provider.display_name(), "Anthropic (claude-sonnet-4-5-20250929)");
    }

    #[tokio::test]
    async fn anthropic_default_and_disabled_thinking_keep_sampling_and_token_limit() {
        for (effort, thinking) in
            [(ReasoningEffort::Default, None), (ReasoningEffort::Disabled, Some(json!({ "type": "disabled" })))]
        {
            let body = anthropic_request_body(effort).await;

            assert_eq!(body.get("thinking"), thinking.as_ref());
            assert_eq!(body["max_tokens"], 500);
            assert_eq!(body["temperature"], 0.5);
            assert_eq!(body["top_p"], 0.5);
        }
    }

    #[tokio::test]
    async fn anthropic_enabled_thinking_clears_sampling_and_raises_token_limit() {
        for (effort, budget_tokens) in
            [(ReasoningEffort::Low, 1024), (ReasoningEffort::High, 10240), (ReasoningEffort::Max, 32768)]
        {
            let body = anthropic_request_body(effort).await;

            assert_eq!(body["thinking"], json!({ "type": "enabled", "budget_tokens": budget_tokens }));
            assert_eq!(body["max_tokens"], budget_tokens + 1024);
            assert!(body.get("temperature").is_none());
            assert!(body.get("top_p").is_none());
        }
    }

    #[tokio::test]
    async fn anthropic_request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_messages().await;
        let provider = GenericProvider::new_with_connection(
            "key",
            &ANTHROPIC,
            ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                request_model: Some("claude-deployment".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert_successful_stream(&responses);
        assert_eq!(captured.body["model"], "claude-deployment");
        assert_eq!(provider.model().unwrap().to_string(), "anthropic:claude-sonnet-4-5-20250929");
    }

    const CHAT_FIXTURE: &str = include_str!("../../tests/fixtures/openai/01_minimal.sse");

    async fn streamed_chat(body: &str) -> Vec<Result<LlmResponse>> {
        let mut server = CaptureServer::start_with_spec(ResponseSpec::sse(body)).await;
        let provider = capture_backed_provider(&server, &OPENROUTER);

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let _ = server.captured().await;
        responses
    }

    fn assert_successful_stream(responses: &[Result<LlmResponse>]) {
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
    }

    fn assert_interrupted(responses: &[Result<LlmResponse>]) {
        let error = responses.last().and_then(|response| response.as_ref().err()).expect("the stream must fail");
        assert_eq!(error.provider().map(|provider| provider.kind), Some(ProviderErrorKind::StreamInterrupted));
        assert!(error.is_retryable());
    }

    async fn anthropic_request_body(effort: ReasoningEffort) -> serde_json::Value {
        let mut server = CaptureServer::start_messages().await;
        let provider = capture_backed_provider(&server, &ANTHROPIC)
            .with_model(&reasoning_off_model(Provider::Anthropic).model_id());
        let mut context = hello_context();
        context.set_reasoning_effort(effort);
        context.set_model_settings(ModelSettings { temperature: Some(0.5), top_p: Some(0.5), max_tokens: Some(500) });

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert_successful_stream(&responses);
        server.captured().await.body
    }

    fn reasoning_off_model(provider: Provider) -> &'static LlmModel {
        LlmModel::all()
            .iter()
            .find(|model| model.provider_enum() == provider && model.supports_reasoning_off())
            .expect("the catalog has a model that can turn reasoning off")
    }

    async fn rejected_chat_request(spec: ResponseSpec) -> LlmError {
        let mut server = CaptureServer::start_with_spec(spec.with_header("x-request-id", "request-123")).await;
        let provider = capture_backed_provider(&server, &OPENROUTER);

        let mut events = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert_eq!(events.len(), 1, "a rejected request must yield exactly one error: {events:?}");
        events.pop().unwrap().expect_err("request must fail")
    }

    fn capture_backed_provider(server: &CaptureServer, config: &'static ProviderConfig) -> GenericProvider {
        GenericProvider::new_with_connection(
            "key",
            config,
            ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                auth_mode: ProviderAuthMode::None,
                ..Default::default()
            },
        )
        .unwrap()
    }
}
