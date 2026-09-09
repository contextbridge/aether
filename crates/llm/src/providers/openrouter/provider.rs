use super::types::OpenRouterChatRequest;
use crate::provider::{error_stream, get_context_window};
use crate::providers::http::openai_client;
use crate::providers::openai_compatible::{
    AetherOpenAiConfig, build_chat_request, streaming::create_custom_stream_generic,
};
use crate::{
    Context, LlmError, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory, Result,
    StreamingModelProvider,
};
use async_openai::{Client, config::OpenAIConfig};
use std::future::ready;

pub struct OpenRouterProvider {
    client: Client<AetherOpenAiConfig>,
    model: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, model: String) -> Result<Self> {
        let config = openai_config(Some(api_key), ProviderConnectionConfig::default());

        let client = openai_client(config, reqwest::Client::new());
        Ok(Self { client, model })
    }

    pub fn default(model: &str) -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| LlmError::MissingApiKey("OPENROUTER_API_KEY".to_string()))?;

        Self::new(api_key, model.to_string())
    }
}

impl ProviderFactory for OpenRouterProvider {
    async fn from_env() -> Result<Self> {
        Self::from_env_with_connection(ProviderConnectionConfig::default()).await
    }

    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = Result<Self>> + Send {
        ready(provider_from_connection(connection))
    }

    fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

impl StreamingModelProvider for OpenRouterProvider {
    fn model(&self) -> Option<crate::LlmModel> {
        format!("openrouter:{}", self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("openrouter", &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let mut request = match build_chat_request(&self.model, context, None) {
            Ok(request) => request,
            Err(e) => return error_stream(e),
        };
        request.prompt_cache_key = context.prompt_cache_key().map(String::from);
        let request = OpenRouterChatRequest::from_compatible(request, context.session_affinity_key());

        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("OpenRouter ({})", self.model)
    }
}

fn openai_config(api_key: Option<String>, connection: ProviderConnectionConfig) -> AetherOpenAiConfig {
    let api_key = api_key.unwrap_or_default();
    let api_base = connection.base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
    let config = OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base);
    AetherOpenAiConfig::new(config, connection.auth_mode)
}

fn provider_from_connection(connection: ProviderConnectionConfig) -> Result<OpenRouterProvider> {
    let api_key = match connection.auth_mode {
        ProviderAuthMode::Default => Some(
            std::env::var("OPENROUTER_API_KEY")
                .map_err(|_| LlmError::MissingApiKey("OPENROUTER_API_KEY".to_string()))?,
        ),
        ProviderAuthMode::None => None,
    };
    let config = openai_config(api_key, connection);
    let client = openai_client(config, reqwest::Client::new());

    Ok(OpenRouterProvider { client, model: String::new() })
}

#[cfg(test)]
mod tests {
    use futures::{StreamExt, stream};
    use reqwest::{Body, Method};

    use super::*;
    use crate::testing::FakeHttpService;
    use crate::{ChatMessage, LlmResponse, ProviderErrorKind};

    #[tokio::test]
    async fn stream_response_propagates_prompt_cache_key_and_keeps_cache_control() {
        let service = FakeHttpService::default();
        service.route(Method::POST, OPENROUTER_URL, || response(200, OPENROUTER_FIXTURE));
        let provider = provider_with_service(&service).with_model("anthropic/claude-haiku-4.5");
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_prompt_cache_key(Some("prefix-abc".to_string()));
        context.set_session_affinity_key(Some("conversation-abc".to_string()));
        context.set_reasoning_effort(Some(crate::ReasoningEffort::High));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let body = request_bodies(&service).pop().unwrap();

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
        assert_eq!(body["model"], "anthropic/claude-haiku-4.5");
        assert_eq!(body["prompt_cache_key"], "prefix-abc");
        assert_eq!(body["session_id"], "conversation-abc");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["cache_control"]["type"], "ephemeral");
        assert_eq!(body["usage"]["include"], true);
    }

    #[tokio::test]
    async fn openrouter_rate_limit_response_is_a_retryable_provider_error() {
        let body = r#"{"error":{"message":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","code":429}}"#;
        let error = rejected_request(429, body).await;
        let provider_error = error.provider().expect("a 429 must classify as a provider error");

        assert_eq!(provider_error.kind, ProviderErrorKind::RateLimit);
        assert_eq!(provider_error.http_status, Some(429));
        assert_eq!(provider_error.code.as_deref(), Some("429"));
        assert_eq!(provider_error.request_id.as_deref(), Some("request-123"));
        assert!(provider_error.message.contains("free-models-per-day"));
        assert!(error.is_retryable(), "429 must be retryable, got {error}");
    }

    #[tokio::test]
    async fn string_error_codes_are_preserved() {
        let error = rejected_request(429, r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#).await;
        let provider_error = error.provider().unwrap();

        assert_eq!(provider_error.code.as_deref(), Some("rate_limit_exceeded"));
        assert!(provider_error.message.contains("slow down"));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn http_status_is_preserved_when_error_body_read_fails() {
        let service = FakeHttpService::default();
        service.route(Method::POST, OPENROUTER_URL, || {
            let body = stream::iter([Err::<String, _>(std::io::Error::other("connection reset"))]);
            response(429, Body::wrap_stream(body))
        });
        let error = collect_rejection(&provider_with_service(&service)).await;
        let provider_error = error.provider().unwrap();

        assert_eq!(provider_error.http_status, Some(429));
        assert_eq!(provider_error.request_id.as_deref(), Some("request-123"));
        assert_eq!(provider_error.kind, ProviderErrorKind::RateLimit);
        assert!(error.is_retryable());
    }

    const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
    const OPENROUTER_FIXTURE: &str = include_str!("../../../tests/fixtures/openrouter/01_minimal.sse");

    fn provider_with_service(service: &FakeHttpService) -> OpenRouterProvider {
        let config = openai_config(Some("test-key".into()), ProviderConnectionConfig::default());
        let client = openai_client(config, service.clone());
        OpenRouterProvider { client, model: "test-model".into() }
    }

    fn response(status: u16, body: impl Into<Body>) -> reqwest::Response {
        http::Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .header("x-request-id", "request-123")
            .body(body.into())
            .unwrap()
            .into()
    }

    fn request_bodies(service: &FakeHttpService) -> Vec<serde_json::Value> {
        service
            .take_requests()
            .iter()
            .map(|request| serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap())
            .collect()
    }

    async fn rejected_request(status: u16, body: &str) -> LlmError {
        let service = FakeHttpService::default();
        let body = body.to_string();
        service.route(Method::POST, OPENROUTER_URL, move || response(status, body.clone()));
        collect_rejection(&provider_with_service(&service)).await
    }

    async fn collect_rejection(provider: &OpenRouterProvider) -> LlmError {
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        let mut events = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert_eq!(events.len(), 1, "a rejected request must yield exactly one error: {events:?}");
        events.pop().unwrap().expect_err("request must fail")
    }
}
