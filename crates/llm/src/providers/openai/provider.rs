use async_openai::{Client, config::Config, types::chat::CreateChatCompletionRequest};
use async_stream;
use std::error::Error;
use tokio_stream::StreamExt;
use tracing::{debug, error};

use super::{
    mappers::{map_messages, map_tools},
    streaming::process_completion_stream,
};
use crate::{Context, LlmError, LlmResponseStream, StreamingModelProvider};

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
        let client = self.client().clone();
        let model = self.model().to_string();
        let prompt_cache_key = context.prompt_cache_key().map(String::from);
        let messages = match map_messages(context.messages()) {
            Ok(messages) => messages,
            Err(e) => return Box::pin(async_stream::stream! { yield Err(e); }),
        };
        let message_count = messages.len();
        let tools = if context.tools().is_empty() {
            None
        } else {
            match map_tools(context.tools(), None) {
                Ok(t) => Some(t),
                Err(e) => return Box::pin(async_stream::stream! { yield Err(e); }),
            }
        };

        Box::pin(async_stream::stream! {
            debug!("Starting chat completion stream for model: {model}");

            let req = CreateChatCompletionRequest {
                model: model.clone(),
                messages,
                tools,
                stream: Some(true),
                prompt_cache_key,
                ..Default::default()
            };

            debug!(
                "Making request to Ollama API with model: {model} and {message_count} messages"
            );

            let stream = match client.chat().create_stream(req).await {
                Ok(stream) => {
                    debug!("Successfully created stream from Ollama API");
                    stream
                }
                Err(e) => {
                    error!("Failed to create stream from Ollama API: {:?}", e);

                    // Check if it's a reqwest error with more details
                    if let Some(reqwest_err) =
                        e.source().and_then(|s| s.downcast_ref::<reqwest::Error>())
                    {
                        if let Some(url) = reqwest_err.url() {
                            error!("Request URL was: {url}");
                        }
                        if let Some(status) = reqwest_err.status() {
                            error!("HTTP status: {status}");
                        }
                    }

                    yield Err(LlmError::ApiRequest(e.to_string()));
                    return;
                }
            };

            let stream = stream.map(|result| {
                result.map_err(|e| LlmError::ApiError(e.to_string()))
            });

            let mut shared_stream = Box::pin(process_completion_stream(stream));
            while let Some(result) = shared_stream.next().await {
                yield result;
            }
        })
    }

    fn context_window(&self) -> Option<u32> {
        None
    }

    fn display_name(&self) -> String {
        let model = self.model();
        if model.is_empty() { self.provider_name().to_string() } else { format!("{} ({model})", self.provider_name()) }
    }
}
