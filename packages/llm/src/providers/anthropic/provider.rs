use super::mappers::{map_messages, map_tools};
use super::streaming::process_anthropic_stream;
use super::types::{Request, Thinking};
use crate::provider::{LlmResponseStream, ProviderFactory, StreamingModelProvider, get_context_window};
use crate::{Context, LlmError, ProviderAuthMode, ProviderConnectionConfig, ReasoningEffort, Result};
use async_stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, header};
use std::env;
use std::time::Duration;
use tracing::debug;

#[derive(Clone)]
pub struct AnthropicProvider {
    client: Client,
    model: String,
    base_url: Option<String>,
    auth_mode: ProviderAuthMode,
    temperature: Option<f32>,
    max_tokens: u32,
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new(api_key: Option<String>) -> Result<Self> {
        let client = build_client()?;

        Ok(Self {
            client,
            model: "claude-sonnet-4-5-20250929".to_string(),
            base_url: Some("https://api.anthropic.com".to_string()),
            auth_mode: ProviderAuthMode::Default,
            temperature: None,
            max_tokens: 16_384,
            api_key,
        })
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
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub(crate) fn build_request(&self, context: &Context) -> Result<Request> {
        let (system_prompt, messages) = map_messages(context.messages())?;
        let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools())?) };

        let mut request = Request::new(self.model.clone(), messages)
            .with_max_tokens(self.max_tokens)
            .with_stream(true)
            .with_auto_caching();

        if let Some(temp) = self.temperature {
            request = request.with_temperature(temp);
        }

        if let Some(system) = system_prompt {
            request = request.with_system_cached(system);
        }

        if let Some(tools) = tools {
            request = request.with_tools(tools);
        }

        if let Some(effort) = context.reasoning_effort() {
            let budget_tokens = effort_to_budget_tokens(effort);
            request = request.with_thinking(Thinking::new(budget_tokens));
            // Anthropic requires temperature to be unset when thinking is enabled
            request.temperature = None;
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
            let api_key = self.get_api_key()?;
            headers.insert("x-api-key", HeaderValue::from_str(&api_key)?);
        }
        Ok(headers)
    }

    async fn send_request(
        &self,
        request: Request,
        headers: header::HeaderMap,
    ) -> Result<impl futures::Stream<Item = Result<String>>> {
        let base_url = self.base_url.as_deref().unwrap_or("https://api.anthropic.com");
        let url = format!("{base_url}/v1/messages");

        debug!("Sending request to Anthropic API: {url}");
        debug!(
            "Anthropic request body: {}",
            serde_json::to_string(&request).unwrap_or_else(|_| "<failed to serialize>".to_string())
        );

        debug!("Anthropic request headers: {}", format_headers(&headers));
        let response = self.client.post(&url).headers(headers).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            let message = format!("Anthropic API request failed with status {status}: {error_text}");
            return Err(match status.as_u16() {
                429 => LlmError::RateLimited(message),
                s if (500..600).contains(&s) => LlmError::ServerError { status: Some(s), message },
                _ => LlmError::ApiError(message),
            });
        }

        let event_stream = response.bytes_stream().eventsource();
        let processed_stream = event_stream.filter_map(|result| {
            std::future::ready(match result {
                Ok(event) => {
                    let data = event.data;
                    if data == "[DONE]" { None } else { Some(Ok(data)) }
                }
                Err(e) => Some(Err(LlmError::StreamInterrupted(e.to_string()))),
            })
        });

        Ok(processed_stream)
    }
}

impl ProviderFactory for AnthropicProvider {
    async fn from_env() -> Result<Self> {
        Self::new(None)
    }

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        Ok(Self::new(None)?.with_connection(connection))
    }

    fn with_model(self, model: &str) -> Self {
        self.with_model(model)
    }
}

impl StreamingModelProvider for AnthropicProvider {
    fn model(&self) -> Option<crate::LlmModel> {
        format!("anthropic:{}", self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("anthropic", &self.model)
    }

    fn stream_response<'a>(&self, context: &Context) -> LlmResponseStream {
        let provider = self.clone();
        let context = context.clone();

        Box::pin(async_stream::stream! {
            let headers = match provider.build_headers() {
                Ok(result) => result,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let request = match provider.build_request(&context) {
                Ok(req) => req,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let stream = match provider.send_request(request, headers).await {
                Ok(stream) => stream,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut anthropic_stream = Box::pin(process_anthropic_stream(stream));
            while let Some(result) = anthropic_stream.next().await {
                yield result;
            }
        })
    }

    fn display_name(&self) -> String {
        format!("Anthropic ({})", self.model)
    }
}

fn build_client() -> Result<Client> {
    Client::builder().timeout(Duration::from_mins(1)).build().map_err(|e| LlmError::HttpClientCreation(e.to_string()))
}

fn effort_to_budget_tokens(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 4096,
        ReasoningEffort::High | ReasoningEffort::Xhigh => 10240,
    }
}

fn should_redact_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization" || lower == "x-api-key" || lower.contains("secret") || lower.contains("token")
}

fn format_headers(headers: &header::HeaderMap) -> String {
    let mut parts = Vec::new();
    for (name, value) in headers {
        let name_str = name.as_str();
        let value_str = if should_redact_header(name_str) {
            "<redacted>".to_string()
        } else {
            value.to_str().unwrap_or("<non-utf8>").to_string()
        };
        parts.push(format!("{name_str}={value_str}"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::ContentBlock;
    use crate::ToolDefinition;
    use crate::providers::anthropic::types::{SystemContent, SystemContentBlock};
    use crate::types::IsoString;
    use reqwest::header::AUTHORIZATION;

    fn create_test_provider() -> AnthropicProvider {
        AnthropicProvider::new(Some("test-api-key".to_string()))
            .unwrap()
            .with_model("claude-sonnet-4-5-20250929")
            .with_temperature(0.7)
            .with_max_tokens(1000)
    }

    #[test]
    fn test_provider_creation() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string()));
        assert!(provider.is_ok());
    }

    #[test]
    fn build_headers_uses_api_key() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())).unwrap();
        let headers = provider.build_headers().expect("headers");
        assert_eq!(headers.get("x-api-key").and_then(|value| value.to_str().ok()), Some("test-api-key"));
        assert!(headers.get(AUTHORIZATION).is_none());
        assert!(headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn build_headers_skips_api_key_when_auth_is_none() {
        let provider = AnthropicProvider::new(None)
            .unwrap()
            .with_connection(ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() });
        let headers = provider.build_headers().expect("headers");
        assert!(headers.get("x-api-key").is_none());
        assert_eq!(headers.get("anthropic-version").and_then(|value| value.to_str().ok()), Some("2023-06-01"));
    }

    #[test]
    fn test_build_request_simple() {
        let provider = create_test_provider();

        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        assert_eq!(request.model, "claude-sonnet-4-5-20250929");
        assert_eq!(request.max_tokens, 1000);
        assert_eq!(request.messages.len(), 1);
        assert!(request.tools.is_none());
        assert!(request.stream);
    }

    #[test]
    fn test_build_request_with_system_and_tools() {
        let provider = create_test_provider();

        let context = Context::new(
            vec![
                ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
            ],
            vec![ToolDefinition {
                name: "search".to_string(),
                description: "Search for information".to_string(),
                parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#.to_string(),
                server: None,
            }],
        );

        let request = provider.build_request(&context).unwrap();
        if let Some(system) = &request.system {
            match system {
                SystemContent::Blocks(blocks) => {
                    assert_eq!(blocks.len(), 1);
                    let SystemContentBlock::Text { text, .. } = &blocks[0];
                    assert_eq!(text, "You are helpful");
                }
                SystemContent::Text(_) => panic!("Expected blocks system content"),
            }
        } else {
            panic!("Expected system prompt");
        }
        assert_eq!(request.messages.len(), 1);
        assert!(request.tools.is_some());
        assert_eq!(request.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_build_request_with_caching() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())).unwrap(); // Caching is enabled by default

        let context = Context::new(
            vec![
                ChatMessage::System { content: "Hello".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
            ],
            vec![ToolDefinition {
                name: "search".to_string(),
                description: "Search for information".to_string(),
                parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#.to_string(),
                server: None,
            }],
        );

        let request = provider.build_request(&context).unwrap();

        // With caching enabled, system prompt should be cached
        if let Some(system) = &request.system {
            match system {
                SystemContent::Blocks(blocks) => {
                    assert_eq!(blocks.len(), 1);
                    let SystemContentBlock::Text { text, cache_control } = &blocks[0];
                    assert_eq!(text, "Hello");
                    assert!(cache_control.is_some());
                }
                SystemContent::Text(_) => panic!("Expected blocks system content for caching"),
            }
        } else {
            panic!("Expected system prompt");
        }

        assert!(request.tools.is_some());

        // Top-level cache_control enables automatic caching
        assert!(request.cache_control.is_some());
    }

    #[test]
    fn test_build_request_with_reasoning_effort() {
        let provider = create_test_provider();

        let mut context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Think hard")], timestamp: IsoString::now() }],
            vec![],
        );
        context.set_reasoning_effort(Some(crate::ReasoningEffort::High));

        let request = provider.build_request(&context).unwrap();
        let thinking = request.thinking.expect("thinking should be set");
        assert_eq!(thinking.thinking_type, "enabled");
        assert_eq!(thinking.budget_tokens, 10240);
        // Temperature must be unset when thinking is enabled
        assert!(request.temperature.is_none());
        // max_tokens must exceed budget_tokens
        assert!(request.max_tokens > thinking.budget_tokens);
    }

    #[test]
    fn test_build_request_without_reasoning_effort_has_no_thinking() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        assert!(request.thinking.is_none());
    }

    #[test]
    fn test_build_request_thinking_bumps_max_tokens_if_needed() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())).unwrap().with_max_tokens(500);

        let mut context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() }],
            vec![],
        );
        context.set_reasoning_effort(Some(crate::ReasoningEffort::Low));

        let request = provider.build_request(&context).unwrap();
        let thinking = request.thinking.as_ref().unwrap();
        assert!(
            request.max_tokens > thinking.budget_tokens,
            "max_tokens ({}) should exceed budget_tokens ({})",
            request.max_tokens,
            thinking.budget_tokens
        );
    }

    #[test]
    fn test_anthropic_provider_display_name() {
        let provider = create_test_provider();
        assert_eq!(provider.display_name(), "Anthropic (claude-sonnet-4-5-20250929)");
    }

    #[test]
    fn test_anthropic_provider_display_name_default() {
        let provider = AnthropicProvider::new(Some("test-api-key".to_string())).unwrap();
        assert_eq!(provider.display_name(), "Anthropic (claude-sonnet-4-5-20250929)");
    }

    #[test]
    fn format_headers_redacts_x_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-secret-123"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let formatted = format_headers(&headers);
        assert!(formatted.contains("x-api-key=<redacted>"));
        assert!(formatted.contains("content-type=application/json"));
        assert!(!formatted.contains("sk-secret-123"));
    }

    #[test]
    fn format_headers_redacts_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer token123"));

        let formatted = format_headers(&headers);
        assert!(formatted.contains("authorization=<redacted>"));
        assert!(!formatted.contains("token123"));
    }

    #[test]
    fn format_headers_redacts_secret_and_token_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-client-secret", HeaderValue::from_static("mysecret"));
        headers.insert("x-auth-token", HeaderValue::from_static("mytoken"));
        headers.insert("accept", HeaderValue::from_static("text/plain"));

        let formatted = format_headers(&headers);
        assert!(formatted.contains("x-client-secret=<redacted>"));
        assert!(formatted.contains("x-auth-token=<redacted>"));
        assert!(formatted.contains("accept=text/plain"));
        assert!(!formatted.contains("mysecret"));
        assert!(!formatted.contains("mytoken"));
    }
}
