use crate::provider::get_context_window;
use crate::providers::openai_compatible::{AetherOpenAiConfig, build_chat_request, create_custom_stream_generic};
use crate::{
    Context, LlmError, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory, Result,
    StreamingModelProvider,
};
use async_stream::stream;
use futures::StreamExt;
use std::env::var;

pub const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

#[derive(Clone)]
pub struct GeminiProvider {
    api_key: Option<String>,
    base_url: Option<String>,
    auth_mode: ProviderAuthMode,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key, base_url: None, auth_mode: ProviderAuthMode::Default, model: String::new() }
    }

    pub fn with_connection(mut self, connection: ProviderConnectionConfig) -> Self {
        self.base_url = connection.base_url;
        self.auth_mode = connection.auth_mode;
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
        async_openai::Client::with_config(AetherOpenAiConfig::new(config, self.auth_mode))
    }
}

impl ProviderFactory for GeminiProvider {
    async fn from_env() -> Result<Self> {
        Ok(Self::new(None))
    }

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        Ok(Self::new(None).with_connection(connection))
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
        let provider = self.clone();
        let context = context.clone();

        Box::pin(stream! {
            let api_key = match provider.get_api_key() {
                Ok(key) => key,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            tracing::info!("Using Gemini API with API key (OpenAI-compatible endpoint)");
            let client = provider.build_openai_client(&api_key);
            let request = match build_chat_request(&provider.model, &context, None) {
                Ok(req) => req,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            let mut inner_stream =
                create_custom_stream_generic(&client, request);

            while let Some(result) = inner_stream.next().await {
                yield result;
            }
        })
    }

    fn display_name(&self) -> String {
        format!("Gemini ({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::config::Config;
    use reqwest::header::AUTHORIZATION;

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
