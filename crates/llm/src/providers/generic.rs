use async_openai::Client;
use async_openai::config::{Config, OpenAIConfig};
use reqwest::Url;
use schemars::Schema;

use crate::catalog::Provider;
use crate::provider::{error_stream, get_context_window, stream_from, validate_reasoning};
use crate::providers::http::openai_client;
use crate::providers::openai_compatible::{
    AetherOpenAiConfig, PromptCacheKeySource, build_chat_request, create_custom_stream_generic,
};
use crate::providers::openai_responses::mappers::build_wire_request;
use crate::providers::openai_responses::transport::{process_connection, send};
use crate::tool_schema::normalize_for_moonshot;
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, Result,
    StreamingModelProvider,
};

pub use crate::providers::openai_responses::mappers::ResponsesRequestPolicy;

pub struct ProviderConfig {
    pub provider: Provider,
    pub api_base: Option<&'static str>,
    pub default_model: &'static str,
    pub api: Api,
}

pub enum Api {
    ChatCompletions { tool_schema_transform: Option<fn(&mut Schema)>, prompt_cache_key: PromptCacheKeySource },
    Responses(ResponsesRequestPolicy),
}

pub const OPENAI: ProviderConfig = ProviderConfig {
    provider: Provider::Openai,
    api_base: Some("https://api.openai.com/v1"),
    default_model: "gpt-4.1",
    api: Api::Responses(ResponsesRequestPolicy::OPENAI),
};

pub const XIAOMI: ProviderConfig = ProviderConfig {
    provider: Provider::Xiaomi,
    api_base: Some("https://api.xiaomimimo.com/v1"),
    default_model: "mimo-v2.6-pro",
    api: Api::Responses(ResponsesRequestPolicy::XIAOMI),
};

pub const DEEPSEEK: ProviderConfig = ProviderConfig {
    provider: Provider::DeepSeek,
    api_base: Some("https://api.deepseek.com"),
    default_model: "deepseek-v4-flash",
    api: Api::ChatCompletions { tool_schema_transform: None, prompt_cache_key: PromptCacheKeySource::Omit },
};

pub const MOONSHOT: ProviderConfig = ProviderConfig {
    provider: Provider::Moonshot,
    api_base: Some("https://api.moonshot.ai/v1"),
    default_model: "moonshot-v1-8k",
    api: Api::ChatCompletions {
        tool_schema_transform: Some(normalize_for_moonshot),
        prompt_cache_key: PromptCacheKeySource::Omit,
    },
};

pub const ZAI: ProviderConfig = ProviderConfig {
    provider: Provider::ZAi,
    api_base: Some("https://api.z.ai/api/coding/paas/v4"),
    default_model: "GLM-4.6",
    api: Api::ChatCompletions { tool_schema_transform: None, prompt_cache_key: PromptCacheKeySource::Omit },
};

pub const AZURE_FOUNDRY: ProviderConfig = ProviderConfig {
    provider: Provider::AzureFoundry,
    api_base: None,
    default_model: "gpt-5.5",
    api: Api::ChatCompletions { tool_schema_transform: None, prompt_cache_key: PromptCacheKeySource::Prefix },
};

pub const FIREWORKS: ProviderConfig = ProviderConfig {
    provider: Provider::Fireworks,
    api_base: Some("https://api.fireworks.ai/inference/v1"),
    default_model: "accounts/fireworks/models/glm-5p1",
    api: Api::ChatCompletions { tool_schema_transform: None, prompt_cache_key: PromptCacheKeySource::SessionAffinity },
};

pub(crate) const BUILT_INS: &[&ProviderConfig] =
    &[&OPENAI, &XIAOMI, &DEEPSEEK, &MOONSHOT, &ZAI, &AZURE_FOUNDRY, &FIREWORKS];

/// A provider whose behavior is fully described by a [`ProviderConfig`].
pub struct GenericProvider {
    config: &'static ProviderConfig,
    openai_config: AetherOpenAiConfig,
    http: reqwest::Client,
    chat_client: Client<AetherOpenAiConfig>,
    model: String,
    request_model: Option<String>,
}

impl GenericProvider {
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
            .ok_or_else(|| LlmError::MissingProviderUrl { provider: config.provider.parser_name().to_string() })?;

        let openai_config = AetherOpenAiConfig::new(
            OpenAIConfig::new().with_api_key(api_key).with_api_base(api_base.trim_end_matches('/')),
            connection.auth_mode,
        );

        let http = reqwest::Client::new();
        Ok(Self {
            config,
            chat_client: openai_client(openai_config.clone(), http.clone()),
            openai_config,
            http,
            model: config.default_model.to_string(),
            request_model: connection.request_model,
        })
    }

    pub fn with_model(mut self, model: &str) -> Self {
        if !model.is_empty() {
            self.model = model.to_string();
        }
        self
    }
}

impl StreamingModelProvider for GenericProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        match &self.config.api {
            Api::ChatCompletions { tool_schema_transform, prompt_cache_key } => {
                self.stream_chat_completions(context, *tool_schema_transform, *prompt_cache_key)
            }
            Api::Responses(policy) => self.stream_responses(context, policy),
        }
    }

    fn display_name(&self) -> String {
        format!("{} ({})", self.config.provider.display_name(), self.model)
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window(self.config.provider.parser_name(), &self.model)
    }

    fn model(&self) -> Option<LlmModel> {
        format!("{}:{}", self.config.provider.parser_name(), self.model).parse().ok()
    }
}

impl GenericProvider {
    fn stream_chat_completions(
        &self,
        context: &Context,
        tool_schema_transform: Option<fn(&mut Schema)>,
        prompt_cache_key: PromptCacheKeySource,
    ) -> LlmResponseStream {
        if let Err(error) = validate_reasoning(context, self.model().as_ref()) {
            return error_stream(error);
        }
        let mut request = match build_chat_request(
            self.request_model.as_deref().unwrap_or(&self.model),
            context,
            tool_schema_transform,
        ) {
            Ok(request) => request,
            Err(error) => return error_stream(error),
        };
        request.prompt_cache_key = prompt_cache_key.resolve(context).map(String::from);
        create_custom_stream_generic(&self.chat_client, request)
    }

    fn stream_responses(&self, context: &Context, policy: &ResponsesRequestPolicy) -> LlmResponseStream {
        let mut url = match Url::parse(&self.openai_config.url("/responses")) {
            Ok(url) => url,
            Err(error) => return error_stream(LlmError::ProviderRequest(error.to_string())),
        };

        url.query_pairs_mut().extend_pairs(self.openai_config.query());

        let mut request = match build_wire_request(&self.model, context, policy) {
            Ok(request) => request,
            Err(error) => return error_stream(error),
        };
        if let Some(model) = &self.request_model {
            request["model"] = model.clone().into();
        }

        let http = self.http.clone();
        let headers = self.openai_config.headers();
        stream_from(async move { send(&http, url.as_str(), headers, request).await }, process_connection)
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::providers::test_capture_server::{CaptureServer, ResponseSpec};
    use crate::testing::FakeHttpService;
    use crate::types::IsoString;
    use crate::{
        AssistantReasoning, ChatMessage, ContentBlock, LlmResponse, MessageId, ProviderErrorKind, ReasoningEffort,
        ToolDefinition,
    };

    #[tokio::test]
    async fn disabled_toggle_and_unknown_models_never_send_requests() {
        let service = FakeHttpService::default();
        let mut provider = GenericProvider::new("key".to_string(), &DEEPSEEK).unwrap();
        provider.chat_client =
            openai_client(AetherOpenAiConfig::new(OpenAIConfig::new(), ProviderAuthMode::None), service.clone());
        for model in ["deepseek-v4-flash", "unknown"] {
            provider = provider.with_model(model);
            let mut context = Context::new(vec![], vec![]);
            context.set_reasoning_effort(ReasoningEffort::Disabled);
            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            assert_eq!(responses.len(), 1);
            let error = responses[0].as_ref().unwrap_err();
            if model == "unknown" {
                assert!(matches!(error, LlmError::ReasoningValidation(_)));
            } else {
                assert!(matches!(error, LlmError::UnsupportedDisableTransport { .. }));
            }
            assert!(!error.is_retryable());
            assert!(service.take_requests().is_empty());
        }
    }

    #[test]
    fn azure_foundry_requires_a_configured_url() {
        let Err(error) = GenericProvider::new("key".to_string(), &AZURE_FOUNDRY) else {
            panic!("Azure Foundry must require a URL");
        };
        assert!(matches!(error, LlmError::MissingProviderUrl { provider } if provider == "azure-foundry"));
    }

    #[tokio::test]
    async fn chat_request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = GenericProvider::new_with_connection(
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
    async fn chat_providers_apply_their_declared_prompt_cache_policy() {
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
    async fn chat_providers_omit_unset_context_keys() {
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

    #[tokio::test]
    async fn openai_distinguishes_default_disabled_and_low_effort() {
        for (effort, expected) in [
            (ReasoningEffort::Default, None),
            (ReasoningEffort::Disabled, Some("none")),
            (ReasoningEffort::Low, Some("low")),
        ] {
            let mut server = CaptureServer::start_responses().await;
            let provider = capture_backed_provider(&server, &OPENAI).with_model("gpt-5.4");
            let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
            context.set_reasoning_effort(effort);

            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            let body = server.captured().await.body;

            assert!(responses.iter().all(Result::is_ok), "{responses:?}");
            assert_eq!(body["reasoning"]["effort"].as_str(), expected);
            if effort == ReasoningEffort::Disabled {
                assert!(body["reasoning"]["summary"].is_null());
            }
        }
    }

    #[tokio::test]
    async fn openai_sends_max_effort_and_prompt_cache_key() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &OPENAI).with_model("gpt-5.6");
        let mut context = Context::new(vec![ChatMessage::user("Think harder")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::Max);
        context.set_prompt_cache_key(Some("cache-key".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert_eq!(captured.body["model"], "gpt-5.6");
        assert_eq!(captured.body["prompt_cache_key"], "cache-key");
        assert_eq!(captured.body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(captured.body["stream"], true);
        assert_eq!(provider.display_name(), "OpenAI (gpt-5.6)");
    }

    #[tokio::test]
    async fn responses_http_200_failed_server_error_is_retryable_with_request_id() {
        let spec = ResponseSpec::sse(include_str!("../../tests/fixtures/openai_responses/04_failed_server.sse"))
            .with_header("x-request-id", "req-openai-1");
        let mut server = CaptureServer::start_with_spec(spec).await;
        let provider = capture_backed_provider(&server, &OPENAI);
        let context = Context::new(vec![ChatMessage::user("hi")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(err.is_retryable(), "server_error must be retryable: {err:?}");
        let provider_error = err.provider().expect("expected provider error");
        assert_eq!(provider_error.kind, ProviderErrorKind::Server);
        assert_eq!(provider_error.http_status, Some(200));
        assert_eq!(provider_error.request_id.as_deref(), Some("req-openai-1"));
        assert_eq!(provider_error.code.as_deref(), Some("server_error"));
    }

    #[tokio::test]
    async fn responses_surface_a_mapping_failure_as_the_only_item() {
        let connection = ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() };
        let provider = GenericProvider::from_env_with_connection(&OPENAI, connection).unwrap();
        let context = Context::new(
            vec![ChatMessage::User {
                message_id: MessageId::new(),
                content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], Err(LlmError::UnsupportedContent(_))), "{responses:?}");
    }

    #[tokio::test]
    async fn xiaomi_omits_encrypted_reasoning_summaries_and_prompt_cache_key() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::High);
        context.set_prompt_cache_key(Some("cache-key".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.path, "/responses");
        assert_eq!(captured.body["model"], "mimo-v2.6-pro");
        assert_eq!(captured.body["reasoning"], json!({ "effort": "high" }));
        assert!(captured.body.get("include").is_none());
        assert!(captured.body.get("prompt_cache_key").is_none());
        assert_eq!(provider.display_name(), "Xiaomi (mimo-v2.6-pro)");
    }

    #[tokio::test]
    async fn xiaomi_replays_prior_reasoning_as_plain_text() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let context = Context::new(
            vec![
                ChatMessage::user("Hello"),
                ChatMessage::Assistant {
                    message_id: MessageId::new(),
                    content: "Hi".to_string(),
                    reasoning: AssistantReasoning::from_parts("greeting the user".to_string(), None),
                    timestamp: IsoString::now(),
                    tool_calls: vec![],
                },
                ChatMessage::user("Again"),
            ],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let reasoning = captured.body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "reasoning")
            .expect("reasoning item should be replayed");
        assert_eq!(reasoning["content"], json!([{ "type": "reasoning_text", "text": "greeting the user" }]));
        assert!(reasoning.get("encrypted_content").is_none_or(serde_json::Value::is_null));
    }

    #[tokio::test]
    async fn xiaomi_drops_null_from_optional_tool_parameters() {
        let mut server = CaptureServer::start_responses().await;
        let provider = capture_backed_provider(&server, &XIAOMI);
        let tool = ToolDefinition::new(
            "bash",
            "Run a command",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "description": { "type": ["string", "null"] }
                },
                "required": ["command"]
            }),
        );
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![tool]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["tools"][0]["parameters"]["properties"]["description"], json!({ "type": "string" }));
    }

    #[tokio::test]
    async fn responses_request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_responses().await;
        let provider = GenericProvider::new_with_connection(
            "key".to_string(),
            &XIAOMI,
            ProviderConnectionConfig {
                base_url: Some(server.base_url.clone()),
                auth_mode: ProviderAuthMode::None,
                request_model: Some("mimo-deployment".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["model"], "mimo-deployment");
        assert_eq!(provider.model().unwrap().to_string(), "xiaomi:mimo-v2.6-pro");
    }

    fn assert_successful_stream(responses: &[Result<LlmResponse>]) {
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
    }

    fn capture_backed_provider(server: &CaptureServer, config: &'static ProviderConfig) -> GenericProvider {
        GenericProvider::new_with_connection(
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
