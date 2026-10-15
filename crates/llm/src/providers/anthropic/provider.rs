use super::mappers::{map_messages, map_tools};
use super::streaming::decode_line;
use super::types::{CacheControl, ContentBlock, Request, Thinking};
use crate::catalog::Provider;
use crate::provider::{LlmResponseStream, ProviderFactory, StreamingModelProvider, display_name, validate_reasoning};
use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
use crate::providers::http::{SseData, anthropic_code, http_client, open_sse};
use crate::providers::response_stream::{OpenedStream, response_stream};
use crate::{Context, LlmError, ProviderAuthMode, ProviderConnectionConfig, ReasoningEffort, Result};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, header};
use std::env;
use std::future::ready;
use std::time::Duration;
use tracing::debug;

/// Tokens requested when no `max_tokens` is set via `ModelSettings`. Anthropic requires
/// `max_tokens` on every request, so a value is always sent.
const DEFAULT_MAX_TOKENS: u32 = 16_384;

#[derive(Clone)]
pub struct AnthropicProvider {
    client: Client,
    model: String,
    base_url: Option<String>,
    auth_mode: ProviderAuthMode,
    api_key: Option<String>,
    idle_timeout: Duration,
}

impl AnthropicProvider {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            client: http_client(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            base_url: Some("https://api.anthropic.com".to_string()),
            auth_mode: ProviderAuthMode::Default,
            api_key,
            idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = Some(base_url.to_string());
        self
    }

    pub fn with_connection(mut self, connection: ProviderConnectionConfig) -> Self {
        if let Some(base_url) = connection.base_url {
            self.base_url = Some(base_url);
        }
        self.auth_mode = connection.auth_mode;
        self.idle_timeout = connection.idle_timeout;
        self
    }

    pub(crate) fn build_request(&self, context: &Context) -> Result<Request> {
        validate_reasoning(context, self.model().as_ref())?;
        let (system_prompt, messages) = map_messages(context.messages())?;
        let settings = context.model_settings();

        let mut request = Request {
            model: self.model.clone(),
            messages,
            max_tokens: settings.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: settings.temperature,
            top_p: settings.top_p,
            system: system_prompt
                .map(|text| vec![ContentBlock::Text { text, cache_control: Some(CacheControl::ephemeral()) }]),
            tools: (!context.tools().is_empty()).then(|| map_tools(context.tools())),
            stream: true,
            thinking: None,
            cache_control: Some(CacheControl::ephemeral()),
        };

        if context.reasoning_effort() == ReasoningEffort::Disabled {
            request.thinking = Some(Thinking::Disabled);
        } else if let Some(budget_tokens) = effort_to_budget_tokens(context.reasoning_effort()) {
            request.thinking = Some(Thinking::Enabled { budget_tokens });
            // Anthropic requires temperature and top_p to be unset when thinking is enabled
            request.temperature = None;
            request.top_p = None;
            // max_tokens must be > budget_tokens
            if request.max_tokens <= budget_tokens {
                request.max_tokens = budget_tokens + 1024;
            }
        }

        debug!("Built Anthropic request for model: {}", request.model);
        Ok(request)
    }

    fn get_api_key(&self) -> Result<String> {
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }

        if let Ok(api_key) = env::var("ANTHROPIC_API_KEY") {
            return Ok(api_key);
        }

        Err(LlmError::MissingApiKey(
            "No Anthropic credentials found. Set ANTHROPIC_API_KEY environment variable.".to_string(),
        ))
    }

    fn build_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.auth_mode != ProviderAuthMode::None {
            let mut api_key = HeaderValue::from_str(&self.get_api_key()?)?;
            api_key.set_sensitive(true);
            headers.insert("x-api-key", api_key);
        }
        Ok(headers)
    }

    async fn send_request(&self, request: Request, headers: header::HeaderMap) -> Result<OpenedStream<SseData>> {
        let base_url = self.base_url.as_deref().unwrap_or("https://api.anthropic.com");
        let url = format!("{base_url}/v1/messages");

        debug!("Sending request to Anthropic API: {url}");
        debug!(
            "Anthropic request body: {}",
            serde_json::to_string(&request).unwrap_or_else(|_| "<failed to serialize>".to_string())
        );

        debug!(?headers, "Anthropic request headers");
        open_sse(self.client.post(&url).headers(headers).json(&request), anthropic_code).await
    }
}

impl ProviderFactory for AnthropicProvider {
    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = Result<Self>> + Send {
        ready(Ok(Self::new(None).with_connection(connection)))
    }

    fn with_model(self, model: &str) -> Self {
        self.with_model(model)
    }
}

impl StreamingModelProvider for AnthropicProvider {
    fn display_name(&self) -> String {
        display_name(Provider::Anthropic, &self.model)
    }

    fn model(&self) -> Option<crate::LlmModel> {
        format!("anthropic:{}", self.model).parse().ok()
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let provider = self.clone();
        let context = context.clone();

        response_stream(
            async move {
                let request = provider.build_request(&context)?;
                let headers = provider.build_headers()?;
                provider.send_request(request, headers).await
            },
            |line, turn| decode_line(&line, turn),
            self.idle_timeout,
        )
    }
}

fn effort_to_budget_tokens(effort: ReasoningEffort) -> Option<u32> {
    Some(match effort {
        ReasoningEffort::Default | ReasoningEffort::Disabled => return None,
        // 1024 is the Anthropic API's minimum thinking budget.
        ReasoningEffort::Minimal | ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 4096,
        ReasoningEffort::High | ReasoningEffort::Xhigh => 10240,
        ReasoningEffort::Max => 32768,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    use crate::ToolDefinition;
    use crate::providers::test_capture_server::{CaptureServer, ResponseSpec, hello_context};
    use futures::StreamExt;

    use reqwest::header::AUTHORIZATION;
    use std::time::Duration;

    fn create_test_provider() -> AnthropicProvider {
        AnthropicProvider::new(Some("test-api-key".to_string())).with_model("claude-sonnet-4-5-20250929")
    }

    #[tokio::test]
    async fn default_and_disabled_thinking_preserve_sampling_and_token_limit() {
        let mut server =
            CaptureServer::start_with_response(include_str!("../../../tests/fixtures/anthropic/01_minimal.sse")).await;
        let model = crate::LlmModel::all()
            .iter()
            .find(|model| {
                model.provider_enum() == crate::catalog::Provider::Anthropic && model.supports_reasoning_off()
            })
            .unwrap();
        let provider = create_test_provider().with_model(&model.model_id()).with_base_url(&server.base_url);
        for (effort, temperature) in [(ReasoningEffort::Default, 0.0), (ReasoningEffort::Disabled, 0.5)] {
            let mut context = hello_context();
            context.set_reasoning_effort(effort);
            context.set_model_settings(crate::ModelSettings {
                temperature: Some(temperature),
                top_p: Some(0.5),
                max_tokens: Some(128),
            });
            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            assert!(responses.iter().all(Result::is_ok), "{responses:?}");
            let body = server.captured().await.body;
            assert_eq!(body["max_tokens"], 128);
            assert_eq!(body["top_p"], 0.5);
            assert_eq!(body["temperature"], serde_json::json!(temperature));
            if effort == ReasoningEffort::Disabled {
                assert_eq!(body["thinking"], serde_json::json!({"type": "disabled"}));
            } else {
                assert!(body.get("thinking").is_none());
            }
        }
    }

    #[tokio::test]
    async fn stream_with_long_pauses_between_events_completes() {
        let spec = ResponseSpec::sse(include_str!("../../../tests/fixtures/anthropic/01_minimal.sse"))
            .paced(Duration::from_mins(2));
        let mut server = CaptureServer::start_with_spec(spec).await;
        let provider = create_test_provider().with_base_url(&server.base_url);

        let responses = server.collect_on_paused_clock(provider.stream_response(&hello_context())).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(crate::LlmResponse::Done { .. }))));
    }

    #[test]
    fn build_headers_uses_api_key() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string()));
        let headers = provider.build_headers().expect("headers");
        assert_eq!(headers.get("x-api-key").and_then(|value| value.to_str().ok()), Some("test-api-key"));
        assert!(headers.get("x-api-key").is_some_and(HeaderValue::is_sensitive));
        assert!(!format!("{headers:?}").contains("test-api-key"));
        assert!(headers.get(AUTHORIZATION).is_none());
        assert!(headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn build_headers_skips_api_key_when_auth_is_none() {
        let provider = AnthropicProvider::new(None)
            .with_connection(ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() });
        let headers = provider.build_headers().expect("headers");
        assert!(headers.get("x-api-key").is_none());
        assert_eq!(headers.get("anthropic-version").and_then(|value| value.to_str().ok()), Some("2023-06-01"));
    }

    #[test]
    fn test_build_request_simple() {
        let provider = create_test_provider();

        let request = provider.build_request(&hello_context()).unwrap();
        assert_eq!(request.model, "claude-sonnet-4-5-20250929");
        assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(request.messages.len(), 1);
        assert!(request.tools.is_none());
        assert!(request.stream);
    }

    #[test]
    fn test_build_request_with_system_and_tools() {
        let provider = create_test_provider();

        let context = Context::new(
            vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")],
            vec![ToolDefinition::new(
                "search",
                "Search for information",
                serde_json::from_str(r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#).unwrap(),
            )],
        );

        let request = provider.build_request(&context).unwrap();
        let Some([ContentBlock::Text { text, .. }]) = request.system.as_deref() else {
            panic!("Expected a single text system block");
        };
        assert_eq!(text, "You are helpful");
        assert_eq!(request.messages.len(), 1);
        assert!(request.tools.is_some());
        assert_eq!(request.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_build_request_with_caching() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())); // Caching is enabled by default

        let context = Context::new(
            vec![ChatMessage::system("Hello"), ChatMessage::user("Hello")],
            vec![ToolDefinition::new(
                "search",
                "Search for information",
                serde_json::from_str(r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#).unwrap(),
            )],
        );

        let request = provider.build_request(&context).unwrap();

        // With caching enabled, system prompt should be cached
        let Some([ContentBlock::Text { text, cache_control }]) = request.system.as_deref() else {
            panic!("Expected a single text system block");
        };
        assert_eq!(text, "Hello");
        assert!(cache_control.is_some());

        assert!(request.tools.is_some());

        // Top-level cache_control enables automatic caching
        assert!(request.cache_control.is_some());
    }

    #[test]
    fn test_build_request_with_reasoning_effort() {
        let provider = create_test_provider();

        let mut context = Context::new(vec![ChatMessage::user("Think hard")], vec![]);
        context.set_reasoning_effort(crate::ReasoningEffort::High);

        let request = provider.build_request(&context).unwrap();
        let Thinking::Enabled { budget_tokens } = request.thinking.unwrap() else {
            panic!("expected enabled thinking")
        };
        assert_eq!(budget_tokens, 10240);
        assert!(request.temperature.is_none());
        assert!(request.max_tokens > budget_tokens);
    }

    #[test]
    fn test_build_request_thinking_clears_sampling() {
        let provider = create_test_provider();
        let mut context = Context::new(vec![ChatMessage::user("Think")], vec![]);
        context.set_model_settings(crate::ModelSettings { temperature: Some(0.2), top_p: Some(0.9), max_tokens: None });
        context.set_reasoning_effort(crate::ReasoningEffort::High);

        let request = provider.build_request(&context).unwrap();
        assert!(request.temperature.is_none());
        assert!(request.top_p.is_none());
    }

    #[test]
    fn test_build_request_thinking_bumps_max_tokens_if_needed() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string()));

        let mut context = Context::new(vec![ChatMessage::user("Hi")], vec![]);
        context.set_model_settings(crate::ModelSettings { max_tokens: Some(500), ..Default::default() });
        context.set_reasoning_effort(crate::ReasoningEffort::Low);

        let request = provider.build_request(&context).unwrap();
        let Thinking::Enabled { budget_tokens } = request.thinking.unwrap() else {
            panic!("expected enabled thinking")
        };
        assert!(request.max_tokens > budget_tokens);
    }

    #[test]
    fn test_anthropic_provider_display_name() {
        let provider = create_test_provider();
        assert_eq!(provider.display_name(), "Anthropic (claude-sonnet-4-5-20250929)");
    }

    #[test]
    fn test_anthropic_provider_display_name_default() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string()));
        assert_eq!(provider.display_name(), "Anthropic (claude-sonnet-4-5-20250929)");
    }

    #[test]
    fn display_name_names_models_outside_the_catalog() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())).with_model("claude-custom");
        assert_eq!(provider.display_name(), "Anthropic (claude-custom)");
    }
}
