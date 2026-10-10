use crate::provider::{get_context_window, validate_reasoning};
use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
use crate::providers::http::{http_client, openai_client};
use crate::providers::openai_compatible::{AetherOpenAiConfig, build_chat_request, create_custom_stream_generic};
use crate::providers::response_stream::error_stream;
use crate::{
    Context, LlmError, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory, Result,
    StreamingModelProvider,
};
use std::env::var;
use std::future::ready;
use std::time::Duration;

pub const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

#[derive(Clone)]
pub struct GeminiProvider {
    api_key: Option<String>,
    base_url: Option<String>,
    auth_mode: ProviderAuthMode,
    model: String,
    http: reqwest::Client,
    idle_timeout: Duration,
}

impl GeminiProvider {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            base_url: None,
            auth_mode: ProviderAuthMode::Default,
            model: String::new(),
            http: http_client(),
            idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }

    pub fn with_connection(mut self, connection: ProviderConnectionConfig) -> Self {
        self.base_url = connection.base_url;
        self.auth_mode = connection.auth_mode;
        self.idle_timeout = connection.idle_timeout;
        self
    }

    fn get_api_key(&self) -> Result<String> {
        if self.auth_mode == ProviderAuthMode::None {
            return Ok(String::new());
        }
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }

        if let Ok(api_key) = var("GEMINI_API_KEY") {
            return Ok(api_key);
        }

        Err(LlmError::MissingApiKey(
            "GEMINI_API_KEY not set. Set the environment variable or provide an API key.".to_string(),
        ))
    }

    fn build_openai_client(&self, api_key: &str) -> async_openai::Client<AetherOpenAiConfig> {
        let api_base = self.base_url.as_deref().unwrap_or(GEMINI_API_BASE);
        let config = async_openai::config::OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base);
        openai_client(AetherOpenAiConfig::new(config, self.auth_mode), self.http.clone())
    }

    fn try_stream_response(&self, context: &Context) -> Result<LlmResponseStream> {
        validate_reasoning(context, self.model().as_ref())?;
        let api_key = self.get_api_key()?;
        let request = build_chat_request(&self.model, context, None)?;

        tracing::info!("Using Gemini API with API key (OpenAI-compatible endpoint)");
        Ok(create_custom_stream_generic(&self.build_openai_client(&api_key), request, self.idle_timeout))
    }
}

impl ProviderFactory for GeminiProvider {
    fn from_env() -> impl Future<Output = Result<Self>> + Send {
        ready(Ok(Self::new(None)))
    }

    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = Result<Self>> + Send {
        ready(Ok(Self::new(None).with_connection(connection)))
    }

    fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

impl StreamingModelProvider for GeminiProvider {
    fn model(&self) -> Option<crate::LlmModel> {
        format!("gemini:{}", self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("gemini", &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        self.try_stream_response(context).unwrap_or_else(error_stream)
    }

    fn display_name(&self) -> String {
        format!("Gemini ({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::config::Config;
    use futures::StreamExt;
    use reqwest::header::AUTHORIZATION;

    #[tokio::test]
    async fn disabled_uses_none_not_minimal() {
        use crate::providers::test_capture_server::CaptureServer;
        let model = crate::LlmModel::all()
            .iter()
            .find(|model| model.provider_enum() == crate::catalog::Provider::Gemini && model.supports_reasoning_off())
            .unwrap();
        let mut server = CaptureServer::start_chat_completions().await;
        let provider =
            GeminiProvider::new(None).with_model(&model.model_id()).with_connection(ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                auth_mode: ProviderAuthMode::None,
                ..Default::default()
            });
        let mut context = Context::new(vec![crate::ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(crate::ReasoningEffort::Disabled);
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(server.captured().await.body["reasoning_effort"], "none");
    }

    #[test]
    fn test_provider_display_name() {
        let provider = GeminiProvider::new(None).with_model("gemini-2.0-flash");
        assert_eq!(provider.display_name(), "Gemini (gemini-2.0-flash)");
    }

    #[test]
    fn get_api_key_returns_empty_when_auth_is_none() {
        let provider = GeminiProvider::new(Some("real-key".to_string()))
            .with_connection(ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() });
        assert_eq!(provider.get_api_key().unwrap(), "");
    }

    #[test]
    fn build_openai_client_strips_authorization_when_auth_is_none() {
        let provider = GeminiProvider::new(Some("real-key".to_string()))
            .with_connection(ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() });
        let api_key = provider.get_api_key().unwrap();
        let client = provider.build_openai_client(&api_key);
        assert!(!client.config().headers().contains_key(AUTHORIZATION));
    }
}
