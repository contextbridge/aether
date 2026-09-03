use super::types::OpenRouterChatRequest;
use crate::provider::{error_stream, get_context_window};
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

        let client = Client::with_config(config);
        Ok(Self { client, model })
    }

    pub fn default(model: &str) -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| LlmError::MissingApiKey("OPENROUTER_API_KEY".to_string()))?;

        let config = openai_config(Some(api_key), ProviderConnectionConfig::default());

        let client = Client::with_config(config);

        Ok(Self { client, model: model.to_string() })
    }
}

fn openai_config(api_key: Option<String>, connection: ProviderConnectionConfig) -> AetherOpenAiConfig {
    let api_key = api_key.unwrap_or_default();
    let api_base = connection.base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
    let config = OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base);
    AetherOpenAiConfig::new(config, connection.auth_mode)
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
        // Build base request and convert to OpenRouter-specific format
        // The From trait automatically adds usage tracking parameters
        // See: https://openrouter.ai/docs/use-cases/usage-accounting
        let mut request: OpenRouterChatRequest = match build_chat_request(&self.model, context, None) {
            Ok(req) => req.into(),
            Err(e) => return error_stream(e),
        };
        request.prompt_cache_key = context.prompt_cache_key().map(String::from);

        if let Some(effort) = context.reasoning_effort() {
            request.reasoning_effort = Some(effort);
        }

        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("OpenRouter ({})", self.model)
    }
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
    let client = Client::with_config(config);

    Ok(OpenRouterProvider { client, model: String::new() })
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::ChatMessage;
    use crate::providers::test_capture_server::CaptureServer;

    #[tokio::test]
    async fn stream_response_propagates_prompt_cache_key_and_keeps_cache_control() {
        let mut server = CaptureServer::start().await;
        let provider = OpenRouterProvider::from_env_with_connection(ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        })
        .await
        .unwrap()
        .with_model("anthropic/claude-haiku-4.5");
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_prompt_cache_key(Some("session-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(!responses.is_empty());
        assert_eq!(captured.path, "/chat/completions");
        assert_eq!(captured.body["prompt_cache_key"], "session-abc");
        assert_eq!(captured.body["cache_control"]["type"], "ephemeral");
    }
}
