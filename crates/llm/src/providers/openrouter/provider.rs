use super::types::OpenRouterChatRequest;
use crate::provider::get_context_window;
use crate::providers::openai_compatible::{
    AetherOpenAiConfig, build_chat_request, streaming::create_custom_stream_generic,
};
use crate::{
    Context, LlmError, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory, Result,
    StreamingModelProvider,
};
use async_openai::{Client, config::OpenAIConfig};

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

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        let api_key = match connection.auth_mode {
            ProviderAuthMode::Default => Some(
                std::env::var("OPENROUTER_API_KEY")
                    .map_err(|_| LlmError::MissingApiKey("OPENROUTER_API_KEY".to_string()))?,
            ),
            ProviderAuthMode::None => None,
        };
        let config = openai_config(api_key, connection);

        let client = Client::with_config(config);

        Ok(Self { client, model: String::new() })
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
            Err(e) => return Box::pin(async_stream::stream! { yield Err(e); }),
        };

        if let Some(effort) = context.reasoning_effort() {
            request.reasoning_effort = Some(effort);
        }

        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("OpenRouter ({})", self.model)
    }
}
