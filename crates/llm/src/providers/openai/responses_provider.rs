use async_openai::config::{Config, OpenAIConfig};
use std::future::ready;

use crate::provider::{error_stream, get_context_window};
use crate::providers::openai_compatible::AetherOpenAiConfig;
use crate::providers::openai_responses::mappers::ResponsesRequestPolicy;
use crate::providers::openai_responses::websocket::{WsRequestParams, derive_ws_url, stream_via_websocket};
use crate::{
    Context, LlmError, LlmModel, LlmResponseStream, ProviderAuthMode, ProviderConnectionConfig, ProviderFactory,
    Result, StreamingModelProvider,
};
use reqwest::Url;

pub struct OpenAiProvider {
    config: AetherOpenAiConfig,
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
        let params = match self.websocket_params() {
            Ok(params) => params,
            Err(error) => return error_stream(error),
        };
        stream_via_websocket(params, self.model.clone(), context.clone())
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

impl OpenAiProvider {
    /// The persistent WebSocket endpoint replaces the per-turn HTTP/SSE
    /// request: same base URL, same headers, upgraded to `ws(s)://`.
    fn websocket_params(&self) -> Result<WsRequestParams> {
        let mut url =
            Url::parse(&self.config.url("/responses")).map_err(|error| LlmError::ProviderRequest(error.to_string()))?;
        url.query_pairs_mut().extend_pairs(self.config.query());
        Ok(WsRequestParams {
            ws_url: derive_ws_url(url.as_str())?,
            handshake_headers: self.config.headers(),
            policy: ResponsesRequestPolicy::openai(),
            on_authentication_failure: None,
        })
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

    Ok(OpenAiProvider { config, model: "gpt-4.1".to_string() })
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
        let captured = server.captured_ws().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert_eq!(captured.body["model"], "gpt-5.6");
        assert_eq!(captured.body["prompt_cache_key"], "cache-key");
        assert_eq!(captured.body["store"], false);
        assert!(captured.body.get("stream").is_none(), "WebSocket envelopes must not carry `stream`");
        assert!(captured.body.get("background").is_none());
        assert!(captured.body.get("previous_response_id").is_none());
    }

    #[tokio::test]
    async fn websocket_error_frames_map_onto_the_provider_taxonomy() {
        use crate::providers::test_capture_server::WsAction;
        let mut server = CaptureServer::start_responses().await;
        server.program_ws(vec![WsAction::Fail {
            status: 500,
            code: "server_error".to_string(),
            message: "The server had an error while processing your request.".to_string(),
        }]);
        let connection = ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        };
        let provider = OpenAiProvider::from_env_with_connection(connection).await.unwrap();
        let context = Context::new(vec![ChatMessage::user("hi")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(crate::LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(err.is_retryable(), "server_error must be retryable: {err:?}");
        let provider_error = err.provider().expect("expected provider error");
        assert_eq!(provider_error.kind, crate::ProviderErrorKind::Server);
        assert_eq!(provider_error.http_status, Some(500));
        assert_eq!(provider_error.code.as_deref(), Some("server_error"));
    }

    #[tokio::test]
    async fn websocket_turns_continue_incrementally_on_the_same_lane() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server).await;

        let mut first_context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        first_context.set_session_affinity_key(Some("conv-1".to_string()));
        let first = provider.stream_response(&first_context).collect::<Vec<_>>().await;
        let first_envelope = server.captured_ws().await;
        assert!(first.iter().all(Result::is_ok), "{first:?}");
        assert!(first_envelope.body.get("previous_response_id").is_none());
        assert_eq!(first_envelope.body["stream_id"], "conv-1");
        assert_eq!(first_envelope.body["input"].as_array().unwrap().len(), 1);

        let mut second_context = Context::new(vec![ChatMessage::user("Hello"), ChatMessage::user("More")], vec![]);
        second_context.set_session_affinity_key(Some("conv-1".to_string()));
        let second = provider.stream_response(&second_context).collect::<Vec<_>>().await;
        let second_envelope = server.captured_ws().await;

        assert!(second.iter().all(Result::is_ok), "{second:?}");
        assert_eq!(second_envelope.body["previous_response_id"], RESPONSE_ID);
        assert_eq!(second_envelope.body["input"].as_array().unwrap().len(), 1);
        assert_eq!(second_envelope.body["input"][0]["content"][0]["text"], "More");
    }

    #[tokio::test]
    async fn compaction_restarts_the_websocket_chain() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server).await;
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        context.set_session_affinity_key(Some("conv-compact".to_string()));

        let first = provider.stream_response(&context).collect::<Vec<_>>().await;
        server.captured_ws().await;
        assert!(first.iter().all(Result::is_ok), "{first:?}");

        let compacted = context.with_compacted_summary("Summary of the conversation");
        let second = provider.stream_response(&compacted).collect::<Vec<_>>().await;
        let second_envelope = server.captured_ws().await;

        assert!(second.iter().all(Result::is_ok), "{second:?}");
        assert!(second_envelope.body.get("previous_response_id").is_none());
        assert_eq!(second_envelope.body["input"].as_array().unwrap().len(), 1);
        assert_eq!(second_envelope.body["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn previous_response_not_found_retries_once_with_the_full_window() {
        use crate::providers::test_capture_server::WsAction;
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server).await;
        let mut first_context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
        first_context.set_session_affinity_key(Some("conv-recover".to_string()));

        let first = provider.stream_response(&first_context).collect::<Vec<_>>().await;
        server.captured_ws().await;
        assert!(first.iter().all(Result::is_ok), "{first:?}");

        // The next continuation hits a server whose connection-local cache no
        // longer knows the lane's response; the client must resend in full.
        server.program_ws(vec![WsAction::Fail {
            status: 400,
            code: "previous_response_not_found".to_string(),
            message: "No cached response for previous_response_id".to_string(),
        }]);

        let mut second_context = Context::new(vec![ChatMessage::user("Hello"), ChatMessage::user("More")], vec![]);
        second_context.set_session_affinity_key(Some("conv-recover".to_string()));
        let second = provider.stream_response(&second_context).collect::<Vec<_>>().await;
        let incremental_attempt = server.captured_ws().await;
        let full_retry = server.captured_ws().await;

        assert!(second.iter().all(Result::is_ok), "{second:?}");
        assert_eq!(incremental_attempt.body["previous_response_id"], RESPONSE_ID);
        assert!(full_retry.body.get("previous_response_id").is_none());
        assert_eq!(full_retry.body["input"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn dropped_connections_reconnect_with_a_full_resend() {
        use crate::providers::test_capture_server::WsAction;
        let mut server = CaptureServer::start_responses().await;
        server.program_ws(vec![WsAction::DropConnection]);
        let provider = server_backed_provider(&server).await;
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let dropped_attempt = server.captured_ws().await;
        let reconnect = server.captured_ws().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(matches!(responses.last(), Some(Ok(crate::LlmResponse::Done { .. }))));
        assert!(dropped_attempt.body.get("previous_response_id").is_none());
        assert!(reconnect.body.get("previous_response_id").is_none());
        assert_eq!(reconnect.body["input"], dropped_attempt.body["input"]);
    }

    async fn server_backed_provider(server: &CaptureServer) -> OpenAiProvider {
        let connection = ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        };
        OpenAiProvider::from_env_with_connection(connection).await.unwrap()
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
        let provider = OpenAiProvider { config, model: "gpt-4.1".to_string() };
        assert_eq!(provider.display_name(), "OpenAI (gpt-4.1)");
    }

    /// Response id of the default fixture, used as the lane's
    /// `previous_response_id` after a completed turn.
    const RESPONSE_ID: &str = "resp_0462ce8cec917eba0069d492bedbe48195a74b5558a4090008";
}
