use async_openai::{Client, config::Config, types::chat::CreateChatCompletionRequest};
use tracing::debug;

use super::mappers::{map_messages, map_tools};
use crate::provider::validate_reasoning;
use crate::providers::openai_compatible::create_custom_stream_generic;
use crate::providers::response_stream::error_stream;
use crate::{Context, LlmResponseStream, Result, StreamingModelProvider};
use std::time::Duration;

/// A Provider that's compatible with `OpenAI`'s chat completion API
/// Other providers (e.g. Ollama, Llama.cpp etc) that are "`OpenAI` compatible" should implement this trait
pub trait OpenAiChatProvider {
    type Config: Config + Clone + 'static;

    fn client(&self) -> &Client<Self::Config>;
    fn model(&self) -> &str;
    fn provider_name(&self) -> &str;
    fn idle_timeout(&self) -> Duration;
}

impl<T: OpenAiChatProvider + Send + Sync> StreamingModelProvider for T {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        try_stream_response(self, context).unwrap_or_else(error_stream)
    }

    fn context_window(&self) -> Option<u32> {
        None
    }

    fn display_name(&self) -> String {
        let model = self.model();
        if model.is_empty() { self.provider_name().to_string() } else { format!("{} ({model})", self.provider_name()) }
    }
}

fn try_stream_response<T: OpenAiChatProvider>(provider: &T, context: &Context) -> Result<LlmResponseStream> {
    validate_reasoning(context, None)?;
    let model = provider.model().to_string();
    let messages = map_messages(context.messages())?;
    let message_count = messages.len();
    let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), None)?) };

    debug!("Starting chat completion stream for model: {model} with {message_count} messages");
    let request = CreateChatCompletionRequest { model, messages, tools, stream: Some(true), ..Default::default() };
    Ok(create_custom_stream_generic(provider.client(), request, provider.idle_timeout()))
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::providers::local::ollama::OllamaProvider;
    use crate::providers::test_capture_server::{CaptureServer, ResponseSpec, hello_context};
    use crate::{LlmError, LlmResponse, ProviderConnectionConfig, ProviderErrorKind, ProviderFactory};

    #[tokio::test]
    async fn local_provider_honors_configured_idle_timeout() {
        let spec = ResponseSpec::sse(include_str!("../../../tests/fixtures/openai/01_minimal.sse"))
            .paced(Duration::from_mins(2));
        let mut server = CaptureServer::start_with_spec(spec).await;
        let connection = ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            idle_timeout: Duration::from_mins(1),
            ..Default::default()
        };
        let provider = OllamaProvider::from_env_with_connection(connection).await.unwrap().with_model("test-model");

        let responses = server.collect_on_paused_clock(provider.stream_response(&hello_context())).await;

        let error = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
        assert_eq!(error.map(|error| error.kind), Some(ProviderErrorKind::Timeout), "{responses:?}");
    }

    #[tokio::test]
    async fn local_chat_providers_omit_cache_metadata() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = OllamaProvider::new("test-model", &server.base_url);
        let mut context = hello_context();
        context.set_prompt_cache_key(Some("prefix-abc".to_string()));
        context.set_session_affinity_key(Some("conversation-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
        assert_eq!(captured.path, "/v1/chat/completions");
        assert!(captured.body.get("prompt_cache_key").is_none());
        assert!(captured.body.get("session_id").is_none());
        assert!(captured.body.get("user").is_none());
    }
}
