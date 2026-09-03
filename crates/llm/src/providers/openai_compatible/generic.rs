use async_openai::{Client, config::OpenAIConfig};
use schemars::Schema;

use crate::catalog::Provider;
use crate::provider::{error_stream, get_context_window};
use crate::tool_schema::normalize_for_moonshot;
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, Result,
    StreamingModelProvider,
};

use super::{AetherOpenAiConfig, PromptCacheKeySource, build_chat_request, create_custom_stream_generic};

/// Configuration for an OpenAI-compatible provider.
///
/// Each provider that uses the standard `build_chat_request → create_custom_stream_generic`
/// flow differs only in these constants.
pub struct ProviderConfig {
    pub provider: Provider,
    pub api_base: Option<&'static str>,
    pub default_model: &'static str,
    pub tool_schema_transform: Option<fn(&mut Schema)>,
    pub prompt_cache_key: PromptCacheKeySource,
}

pub const DEEPSEEK: ProviderConfig = ProviderConfig {
    provider: Provider::DeepSeek,
    api_base: Some("https://api.deepseek.com"),
    default_model: "deepseek-v4-flash",
    tool_schema_transform: None,
    prompt_cache_key: PromptCacheKeySource::Omit,
};

pub const MOONSHOT: ProviderConfig = ProviderConfig {
    provider: Provider::Moonshot,
    api_base: Some("https://api.moonshot.ai/v1"),
    default_model: "moonshot-v1-8k",
    tool_schema_transform: Some(normalize_for_moonshot),
    prompt_cache_key: PromptCacheKeySource::Omit,
};

pub const ZAI: ProviderConfig = ProviderConfig {
    provider: Provider::ZAi,
    api_base: Some("https://api.z.ai/api/coding/paas/v4"),
    default_model: "GLM-4.6",
    tool_schema_transform: None,
    prompt_cache_key: PromptCacheKeySource::Omit,
};

pub const AZURE_FOUNDRY: ProviderConfig = ProviderConfig {
    provider: Provider::AzureFoundry,
    api_base: None,
    default_model: "gpt-5.5",
    tool_schema_transform: None,
    prompt_cache_key: PromptCacheKeySource::Prefix,
};

pub const FIREWORKS: ProviderConfig = ProviderConfig {
    provider: Provider::Fireworks,
    api_base: Some("https://api.fireworks.ai/inference/v1"),
    default_model: "accounts/fireworks/models/glm-5p1",
    tool_schema_transform: None,
    prompt_cache_key: PromptCacheKeySource::SessionAffinity,
};

pub(crate) const BUILT_INS: &[&ProviderConfig] = &[&DEEPSEEK, &MOONSHOT, &ZAI, &AZURE_FOUNDRY, &FIREWORKS];

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
                let env_var = config.provider.required_env_var().expect("generic providers require an API key");
                std::env::var(env_var).map_err(|_| LlmError::MissingApiKey(env_var.to_string()))?
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
            .ok_or_else(|| LlmError::MissingProviderUrl { provider: config.provider.parser_name().to_string() })?
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
        format!("{}:{}", self.config.provider.parser_name(), self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window(self.config.provider.parser_name(), &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let mut request = match build_chat_request(
            self.request_model.as_deref().unwrap_or(&self.model),
            context,
            self.config.tool_schema_transform,
        ) {
            Ok(req) => req,
            Err(e) => return error_stream(e),
        };
        request.prompt_cache_key = self.config.prompt_cache_key.resolve(context).map(String::from);
        create_custom_stream_generic(&self.client, request)
    }

    fn display_name(&self) -> String {
        format!("{} ({})", self.config.provider.display_name(), self.model)
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
        let mut server = CaptureServer::start_chat_completions().await;
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
    async fn providers_apply_their_declared_prompt_cache_policy() {
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
    async fn providers_omit_unset_context_keys() {
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

    fn assert_successful_stream(responses: &[Result<crate::LlmResponse>]) {
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(crate::LlmResponse::Done { .. }))));
    }

    fn capture_backed_provider(server: &CaptureServer, config: &'static ProviderConfig) -> GenericOpenAiProvider {
        GenericOpenAiProvider::new_with_connection(
            "key".to_string(),
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
