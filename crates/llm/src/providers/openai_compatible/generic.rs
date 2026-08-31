use async_openai::{Client, config::OpenAIConfig};
use schemars::Schema;

use crate::provider::{error_stream, get_context_window};
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
    pub api_base: Option<&'static str>,
    pub env_var: &'static str,
    pub default_model: &'static str,
    pub prefix: &'static str,
    pub display_name: &'static str,
    pub tool_schema_transform: Option<fn(&mut Schema)>,
}

pub const DEEPSEEK: ProviderConfig = ProviderConfig {
    api_base: Some("https://api.deepseek.com"),
    env_var: "DEEPSEEK_API_KEY",
    default_model: "deepseek-v4-flash",
    prefix: "deepseek",
    display_name: "DeepSeek",
    tool_schema_transform: None,
};

pub const MOONSHOT: ProviderConfig = ProviderConfig {
    api_base: Some("https://api.moonshot.ai/v1"),
    env_var: "MOONSHOT_API_KEY",
    default_model: "moonshot-v1-8k",
    prefix: "moonshot",
    display_name: "Moonshot",
    tool_schema_transform: Some(normalize_for_moonshot),
};

pub const ZAI: ProviderConfig = ProviderConfig {
    api_base: Some("https://api.z.ai/api/coding/paas/v4"),
    env_var: "ZAI_API_KEY",
    default_model: "GLM-4.6",
    prefix: "zai",
    display_name: "Z.ai",
    tool_schema_transform: None,
};

pub const AZURE_FOUNDRY: ProviderConfig = ProviderConfig {
    api_base: None,
    env_var: "AZURE_OPENAI_API_KEY",
    default_model: "gpt-5.5",
    prefix: "azure-foundry",
    display_name: "Microsoft Foundry",
    tool_schema_transform: None,
};

pub const FIREWORKS: ProviderConfig = ProviderConfig {
    api_base: Some("https://api.fireworks.ai/inference/v1"),
    env_var: "FIREWORKS_API_KEY",
    default_model: "accounts/fireworks/models/glm-5p1",
    prefix: "fireworks",
    display_name: "Fireworks AI",
    tool_schema_transform: None,
};

/// A generic provider for APIs that are fully OpenAI-compatible.
pub struct GenericOpenAiProvider {
    client: Client<AetherOpenAiConfig>,
    model: String,
    request_model: Option<String>,
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
        Self::new_with_connection(api_key, config, connection)
    }

    pub fn new(api_key: String, config: &'static ProviderConfig) -> Result<Self> {
        Self::new_with_connection(api_key, config, ProviderConnectionConfig::default())
    }

    pub fn new_with_connection(
        api_key: String,
        config: &'static ProviderConfig,
        connection: ProviderConnectionConfig,
    ) -> Result<Self> {
        let api_base = connection
            .base_url
            .or_else(|| config.api_base.map(str::to_string))
            .ok_or_else(|| LlmError::MissingProviderUrl { provider: config.prefix.to_string() })?
            .trim_end_matches('/')
            .to_string();
        let openai_config = OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base);
        let openai_config = AetherOpenAiConfig::new(openai_config, connection.auth_mode);

        Ok(Self {
            client: Client::with_config(openai_config),
            model: config.default_model.to_string(),
            request_model: connection.request_model,
            config,
        })
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
        let request = match build_chat_request(
            self.request_model.as_deref().unwrap_or(&self.model),
            context,
            self.config.tool_schema_transform,
        ) {
            Ok(req) => req,
            Err(e) => return error_stream(e),
        };
        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("{} ({})", self.config.display_name, self.model)
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::providers::test_capture_server::CaptureServer;

    use crate::ChatMessage;

    #[test]
    fn azure_foundry_requires_a_configured_url() {
        let Err(error) = GenericOpenAiProvider::new("key".to_string(), &AZURE_FOUNDRY) else {
            panic!("Azure Foundry must require a URL");
        };
        assert!(matches!(error, LlmError::MissingProviderUrl { provider } if provider == "azure-foundry"));
    }

    #[tokio::test]
    async fn request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start().await;
        let provider = GenericOpenAiProvider::new_with_connection(
            "key".to_string(),
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

        assert!(!responses.is_empty());
        assert_eq!(captured.path, "/chat/completions");
        assert_eq!(captured.body["model"], "production-coding");
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.body["stream_options"]["include_usage"], true);
        assert!(captured.headers.get("authorization").is_none());
        assert_eq!(provider.model().unwrap().to_string(), "azure-foundry:gpt-5.5");
        assert_eq!(provider.display_name(), "Microsoft Foundry (gpt-5.5)");
    }
}
