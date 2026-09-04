use async_openai::config::{Config, OpenAIConfig};
use std::future::ready;
use tracing::debug;

use crate::provider::{error_stream, get_context_window, stream_from};
use crate::providers::openai_compatible::AetherOpenAiConfig;
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_wire_request};
use crate::providers::openai_responses::transport::{process_connection, send};
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory,
    Result, StreamingModelProvider,
};
use reqwest::Url;

pub struct OpenAiProvider {
    config: AetherOpenAiConfig,
    http: reqwest::Client,
    model: String,
}

impl ProviderFactory for OpenAiProvider {
    async fn from_env() -> Result<Self> {
        Self::from_env_with_connection(ProviderConnectionConfig::default()).await
    }

    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = Result<Self>> + Send {
        ready(provider_from_connection(connection))
    }

    fn with_model(mut self, model: &str) -> Self {
        if !model.is_empty() {
            self.model = model.to_string();
        }
        self
    }
}

impl StreamingModelProvider for OpenAiProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let http = self.http.clone();
        let mut url = match Url::parse(&self.config.url("/responses")) {
            Ok(url) => url,
            Err(error) => return error_stream(LlmError::ProviderRequest(error.to_string())),
        };
        url.query_pairs_mut().extend_pairs(self.config.query());
        let url = url.to_string();
        let headers = self.config.headers();
        let model = self.model.clone();
        let request = match build_wire_request(&model, context, &ResponsesRequestPolicy::openai()) {
            Ok(request) => request,
            Err(e) => return error_stream(e),
        };

        stream_from(
            async move {
                debug!("Starting OpenAI Responses API stream for model: {model}");
                send(&http, &url, headers, request).await
            },
            process_connection,
        )
    }

    fn display_name(&self) -> String {
        format!("OpenAI ({})", self.model)
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("openai", &self.model)
    }

    fn model(&self) -> Option<LlmModel> {
        format!("openai:{}", self.model).parse().ok()
    }
}

fn provider_from_connection(connection: ProviderConnectionConfig) -> Result<OpenAiProvider> {
    let api_key = match connection.auth_mode {
        ProviderAuthMode::Default => {
            std::env::var("OPENAI_API_KEY").map_err(|_| LlmError::MissingApiKey("OPENAI_API_KEY".to_string()))?
        }
        ProviderAuthMode::None => String::new(),
    };

    let mut config = OpenAIConfig::new().with_api_key(api_key);
    if let Some(base_url) = connection.base_url {
        config = config.with_api_base(base_url);
    }
    let config = AetherOpenAiConfig::new(config, connection.auth_mode);
    let http = reqwest::Client::new();

    Ok(OpenAiProvider { config, http, model: "gpt-4.1".to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_capture_server::CaptureServer;
    use crate::{ChatMessage, ReasoningEffort};
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn stream_response_sends_max_effort_on_the_wire() {
        let mut server = CaptureServer::start_responses().await;
        let connection = ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        };
        let provider = OpenAiProvider::from_env_with_connection(connection).await.unwrap().with_model("gpt-5.6");
        let mut context = Context::new(vec![ChatMessage::user("Think harder")], vec![]);
        context.set_reasoning_effort(Some(ReasoningEffort::Max));
        context.set_prompt_cache_key(Some("cache-key".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert_eq!(captured.body["model"], "gpt-5.6");
        assert_eq!(captured.body["prompt_cache_key"], "cache-key");
        assert_eq!(captured.body["stream"], true);
    }

    #[tokio::test]
    async fn http_200_failed_server_error_is_retryable_with_request_id() {
        use crate::providers::test_capture_server::ResponseSpec;
        let spec = ResponseSpec::sse(include_str!("../../../tests/fixtures/openai_responses/04_failed_server.sse"))
            .with_header("x-request-id", "req-openai-1");
        let mut server = CaptureServer::start_with_spec(spec).await;
        let connection = ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        };
        let provider = OpenAiProvider::from_env_with_connection(connection).await.unwrap();
        let context = Context::new(vec![ChatMessage::user("hi")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(crate::LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(err.is_retryable(), "server_error must be retryable: {err:?}");
        let provider_error = err.provider().expect("expected provider error");
        assert_eq!(provider_error.kind, crate::ProviderErrorKind::Server);
        assert_eq!(provider_error.http_status, Some(200));
        assert_eq!(provider_error.request_id.as_deref(), Some("req-openai-1"));
        assert_eq!(provider_error.code.as_deref(), Some("server_error"));
    }

    #[tokio::test]
    async fn stream_response_surfaces_a_mapping_failure_as_the_only_item() {
        let connection = ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() };
        let provider = OpenAiProvider::from_env_with_connection(connection).await.unwrap();
        let context = Context::new(
            vec![ChatMessage::User {
                content: vec![crate::ContentBlock::Audio {
                    data: "YXVkaW8=".to_string(),
                    mime_type: "audio/wav".to_string(),
                }],
                timestamp: crate::types::IsoString::now(),
            }],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0], Err(LlmError::UnsupportedContent(_))), "{responses:?}");
    }

    #[test]
    fn test_provider_display_name() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("test"), ProviderAuthMode::Default);
        let provider = OpenAiProvider { config, http: reqwest::Client::new(), model: "gpt-4.1".to_string() };
        assert_eq!(provider.display_name(), "OpenAI (gpt-4.1)");
    }
}
