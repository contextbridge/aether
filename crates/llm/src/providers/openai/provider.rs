use async_openai::{Client, config::Config, types::chat::CreateChatCompletionRequest};
use tracing::debug;

use super::mappers::{map_messages, map_tools};
use crate::provider::error_stream;
use crate::providers::openai_compatible::create_custom_stream_generic;
use crate::{Context, LlmResponseStream, StreamingModelProvider};

/// A Provider that's compatible with `OpenAI`'s chat completion API
/// Other providers (e.g. Ollama, Llama.cpp etc) that are "`OpenAI` compatible" should implement this trait
pub trait OpenAiChatProvider {
    type Config: Config + Clone + 'static;

    fn client(&self) -> &Client<Self::Config>;
    fn model(&self) -> &str;
    fn provider_name(&self) -> &str;
}

impl<T: OpenAiChatProvider + Send + Sync> StreamingModelProvider for T {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        if let Err(error) = crate::provider::validate_reasoning(context, None) {
            return crate::provider::error_stream(error);
        }
        let model = self.model().to_string();
        let messages = match map_messages(context.messages()) {
            Ok(messages) => messages,
            Err(e) => return error_stream(e),
        };
        let message_count = messages.len();
        let tools = if context.tools().is_empty() {
            None
        } else {
            match map_tools(context.tools(), None) {
                Ok(t) => Some(t),
                Err(e) => return error_stream(e),
            }
        };

        debug!("Starting chat completion stream for model: {model} with {message_count} messages");
        let request = CreateChatCompletionRequest { model, messages, tools, stream: Some(true), ..Default::default() };
        create_custom_stream_generic(self.client(), request)
    }

    fn context_window(&self) -> Option<u32> {
        None
    }

    fn display_name(&self) -> String {
        let model = self.model();
        if model.is_empty() { self.provider_name().to_string() } else { format!("{} ({model})", self.provider_name()) }
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::providers::local::ollama::OllamaProvider;
    use crate::providers::test_capture_server::CaptureServer;
    use crate::{ChatMessage, LlmResponse, Result};

    #[tokio::test]
    async fn local_chat_providers_omit_cache_metadata() {
        let mut server = CaptureServer::start_chat_completions().await;
        let provider = OllamaProvider::new("test-model", &server.base_url);
        let mut context = Context::new(vec![ChatMessage::user("Hello")], vec![]);
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
