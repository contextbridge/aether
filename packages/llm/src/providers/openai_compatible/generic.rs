use async_openai::{Client, config::OpenAIConfig};
use schemars::Schema;

use crate::provider::get_context_window;
use crate::tool_schema::normalize_for_moonshot;
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, Result,
    StreamingModelProvider,
};

use super::{AetherOpenAiConfig, build_chat_request, create_custom_stream_generic};

/// Configuration for an OpenAI-compatible provider.
///
/// Each provider that uses the standard `build_chat_request → create_custom_stream_generic`
/// flow differs only in these constants.
pub struct ProviderConfig {
    pub api_base: &'static str,
    pub env_var: &'static str,
    pub default_model: &'static str,
    pub prefix: &'static str,
    pub display_name: &'static str,
    pub tool_schema_transform: Option<fn(&mut Schema)>,
}

pub const DEEPSEEK: ProviderConfig = ProviderConfig {
    api_base: "https://api.deepseek.com",
    env_var: "DEEPSEEK_API_KEY",
    default_model: "deepseek-chat",
    prefix: "deepseek",
    display_name: "DeepSeek",
    tool_schema_transform: None,
};

pub const MOONSHOT: ProviderConfig = ProviderConfig {
    api_base: "https://api.moonshot.ai/v1",
    env_var: "MOONSHOT_API_KEY",
    default_model: "moonshot-v1-8k",
    prefix: "moonshot",
    display_name: "Moonshot",
    tool_schema_transform: Some(normalize_for_moonshot),
};

pub const ZAI: ProviderConfig = ProviderConfig {
    api_base: "https://api.z.ai/api/coding/paas/v4",
    env_var: "ZAI_API_KEY",
    default_model: "GLM-4.6",
    prefix: "zai",
    display_name: "Z.ai",
    tool_schema_transform: None,
};

/// A generic provider for APIs that are fully OpenAI-compatible.
pub struct GenericOpenAiProvider {
    client: Client<AetherOpenAiConfig>,
    model: String,
    config: &'static ProviderConfig,
}

impl GenericOpenAiProvider {
    pub fn from_env(config: &'static ProviderConfig) -> Result<Self> {
        Self::from_env_with_connection(config, ProviderConnectionConfig::default())
    }

    pub fn from_env_with_connection(
        config: &'static ProviderConfig,
        connection: ProviderConnectionConfig,
    ) -> Result<Self> {
        let api_key = match connection.auth_mode {
            ProviderAuthMode::Default => {
                std::env::var(config.env_var).map_err(|_| LlmError::MissingApiKey(config.env_var.to_string()))?
            }
            ProviderAuthMode::None => String::new(),
        };
        Ok(Self::new_with_connection(api_key, config, connection))
    }

    pub fn new(api_key: String, config: &'static ProviderConfig) -> Self {
        Self::new_with_connection(api_key, config, ProviderConnectionConfig::default())
    }

    pub fn new_with_connection(
        api_key: String,
        config: &'static ProviderConfig,
        connection: ProviderConnectionConfig,
    ) -> Self {
        let api_base = connection.base_url.unwrap_or_else(|| config.api_base.to_string());
        let openai_config = OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base);
        let openai_config = AetherOpenAiConfig::new(openai_config, connection.auth_mode);

        Self { client: Client::with_config(openai_config), model: config.default_model.to_string(), config }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

impl StreamingModelProvider for GenericOpenAiProvider {
    fn model(&self) -> Option<LlmModel> {
        format!("{}:{}", self.config.prefix, self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window(self.config.prefix, &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let request = match build_chat_request(&self.model, context, self.config.tool_schema_transform) {
            Ok(req) => req,
            Err(e) => return Box::pin(async_stream::stream! { yield Err(e); }),
        };
        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("{} ({})", self.config.display_name, self.model)
    }
}
