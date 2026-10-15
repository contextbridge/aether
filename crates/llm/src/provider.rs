use crate::LlmError;
use crate::LlmModel;
use crate::ProviderConnectionConfig;
use crate::Result as LlmResult;
use crate::catalog::{Provider, ReasoningEffortError};
use std::future::Future;
use std::pin::Pin;
use tokio_stream::Stream;
use utils::ReasoningEffort;

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
    /// Create provider from environment variables with provider connection overrides.
    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = LlmResult<Self>> + Send;

    /// Create provider from environment variables and default configuration
    fn from_env() -> impl Future<Output = LlmResult<Self>> + Send {
        Self::from_env_with_connection(ProviderConnectionConfig::default())
    }

    /// Set or update the model for this provider (builder pattern)
    fn with_model(self, model: &str) -> Self;
}

#[doc = include_str!("docs/streaming_model_provider.md")]
pub trait StreamingModelProvider: Send + Sync {
    fn stream_response(&self, context: &Context) -> LlmResponseStream;

    /// Provider and model, e.g. `Anthropic (claude-sonnet-4-5)`.
    fn display_name(&self) -> String {
        self.model()
            .map_or_else(|| "Unknown model".to_string(), |model| display_name(model.provider_enum(), &model.model_id()))
    }

    /// Context window size in tokens for the current model.
    /// Returns `None` for unknown models (e.g. Ollama, `LlamaCpp`).
    fn context_window(&self) -> Option<u32> {
        self.model().and_then(|model| model.context_window())
    }

    /// The `LlmModel` this provider is currently configured to use.
    /// Returns `None` for providers where the model is unknown at compile time
    /// (e.g. test fakes).
    fn model(&self) -> Option<LlmModel> {
        None
    }
}

pub(crate) fn display_name(provider: Provider, model_id: &str) -> String {
    if model_id.is_empty() {
        provider.display_name().to_string()
    } else {
        format!("{} ({model_id})", provider.display_name())
    }
}

pub(crate) fn validate_reasoning(context: &Context, model: Option<&LlmModel>) -> LlmResult<()> {
    if context.reasoning_effort() != ReasoningEffort::Disabled {
        return Ok(());
    }

    let model = model.ok_or_else(|| ReasoningEffortError::Unsupported {
        model: "unknown".to_string(),
        effort: ReasoningEffort::Disabled,
        supported: Vec::new(),
    })?;

    if !model.supports_reasoning_off() {
        model.validate_reasoning_effort(ReasoningEffort::Disabled)?;
    }

    if !model.supports_reasoning_off_transport() {
        return Err(LlmError::UnsupportedDisableTransport { model: model.to_string() });
    }

    Ok(())
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
