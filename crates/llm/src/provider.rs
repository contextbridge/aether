use crate::LlmError;
use crate::LlmModel;
use crate::ProviderConnectionConfig;
use crate::Result as LlmResult;
use std::future::Future;
use std::pin::Pin;
use tokio_stream::{Stream, StreamExt};

use super::{Context, LlmResponse};

/// A stream of [`LlmResponse`] events from an LLM provider.
///
/// This is a pinned, boxed, `Send` stream used as the return type of
/// [`StreamingModelProvider::stream_response`]. Boxing is required to support
/// trait objects (`Vec<Box<dyn StreamingModelProvider>>`) in types like
/// [`AlloyedModelProvider`](crate::alloyed::AlloyedModelProvider).
pub type LlmResponseStream = Pin<Box<dyn Stream<Item = LlmResult<LlmResponse>> + Send>>;

#[doc = include_str!("docs/provider_factory.md")]
pub trait ProviderFactory: Sized {
    /// Create provider from environment variables and default configuration
    fn from_env() -> impl Future<Output = LlmResult<Self>> + Send;

    /// Create provider from environment variables with provider connection overrides.
    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = LlmResult<Self>> + Send {
        async move {
            let _ = connection;
            Self::from_env().await
        }
    }

    /// Set or update the model for this provider (builder pattern)
    fn with_model(self, model: &str) -> Self;
}

#[doc = include_str!("docs/streaming_model_provider.md")]
pub trait StreamingModelProvider: Send + Sync {
    fn stream_response(&self, context: &Context) -> LlmResponseStream;
    fn display_name(&self) -> String;

    /// Context window size in tokens for the current model.
    /// Returns `None` for unknown models (e.g. Ollama, `LlamaCpp`).
    fn context_window(&self) -> Option<u32>;

    /// The `LlmModel` this provider is currently configured to use.
    /// Returns `None` for providers where the model is unknown at compile time
    /// (e.g. test fakes).
    fn model(&self) -> Option<LlmModel> {
        None
    }
}

/// Look up context window for a known provider + model ID combo via the catalog.
///
/// Returns `None` if the model is not in the catalog.
pub fn get_context_window(provider: &str, model_id: &str) -> Option<u32> {
    let key = format!("{provider}:{model_id}");
    key.parse::<LlmModel>().ok().and_then(|m| m.context_window())
}

/// Bridge a fallible request setup into an [`LlmResponseStream`].
///
/// `open` issues the request; `process` turns what it returns into a response
/// stream. A setup failure becomes the stream's single item, so providers never
/// hand-roll the yield-then-return dance and cannot drop an error on the way.
pub(crate) fn stream_from<T, S>(
    open: impl Future<Output = LlmResult<T>> + Send + 'static,
    process: impl FnOnce(T) -> S + Send + 'static,
) -> LlmResponseStream
where
    T: Send,
    S: Stream<Item = LlmResult<LlmResponse>> + Send + 'static,
{
    Box::pin(async_stream::stream! {
        match open.await {
            Ok(opened) => {
                let mut stream = Box::pin(process(opened));
                while let Some(item) = stream.next().await {
                    yield item;
                }
            }
            Err(error) => yield Err(error),
        }
    })
}

/// A response stream whose only item is `error`.
pub(crate) fn error_stream(error: LlmError) -> LlmResponseStream {
    Box::pin(tokio_stream::once(Err(error)))
}

impl StreamingModelProvider for Box<dyn StreamingModelProvider> {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        (**self).stream_response(context)
    }

    fn display_name(&self) -> String {
        (**self).display_name()
    }

    fn context_window(&self) -> Option<u32> {
        (**self).context_window()
    }

    fn model(&self) -> Option<LlmModel> {
        (**self).model()
    }
}

impl<T: StreamingModelProvider + ?Sized> StreamingModelProvider for std::sync::Arc<T> {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        (**self).stream_response(context)
    }

    fn display_name(&self) -> String {
        (**self).display_name()
    }

    fn context_window(&self) -> Option<u32> {
        (**self).context_window()
    }

    fn model(&self) -> Option<LlmModel> {
        (**self).model()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_context_window_known_model() {
        assert_eq!(get_context_window("anthropic", "claude-opus-4-6"), Some(1_000_000));
    }

    #[test]
    fn lookup_context_window_openrouter_model() {
        // OpenRouter Qwen models should resolve from catalog
        let result = get_context_window("openrouter", "anthropic/claude-opus-4");
        assert_eq!(result, Some(200_000));
    }

    #[test]
    fn lookup_context_window_unknown_model() {
        assert_eq!(get_context_window("anthropic", "unknown-model-xyz"), None);
    }

    #[test]
    fn lookup_context_window_unknown_provider() {
        assert_eq!(get_context_window("unknown-provider", "some-model"), None);
    }
}
