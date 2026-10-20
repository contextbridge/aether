use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError;
use aws_sdk_bedrockruntime::operation::invoke_model_with_response_stream::InvokeModelWithResponseStreamError;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::{ConverseStreamOutputError, ResponseStreamError};
use aws_smithy_types::event_stream::RawMessage;
use futures::{Stream, stream};
use std::fmt::Debug;
use std::time::Duration;
use tracing::error;

use super::provider::ModelTarget;
use crate::providers::response_stream::{OpenedStream, StreamAssembler, response_stream};
use crate::{LlmError, LlmResponse, LlmResponseStream, ProviderError, Result};

pub(super) trait BedrockException {
    fn kind(&self) -> ExceptionKind;
}

pub(super) enum ExceptionKind {
    Throttled,
    Transient,
    Other,
}

pub(super) fn stream<T, U, V>(
    send: impl Future<Output = Result<EventReceiver<T, U>>> + Send + 'static,
    decode: impl FnMut(T, &mut StreamAssembler<V>) -> Result<Vec<LlmResponse>> + Send + 'static,
    idle_timeout: Duration,
) -> LlmResponseStream
where
    T: Send + 'static,
    U: BedrockException + Send + 'static,
    V: Ord + Debug + Send + 'static,
{
    response_stream(async move { Ok(OpenedStream::new(events(send.await?))) }, decode, idle_timeout)
}

pub(super) fn request_error<T: BedrockException + Debug>(target: &ModelTarget, error: &SdkError<T>) -> LlmError {
    error!(model = %target.model, error = ?error, "Bedrock API error");
    let message = format!("Bedrock API error: {error}");
    let status = error.raw_response().map(|response| response.status().as_u16());
    let request_id =
        error.raw_response().and_then(|response| response.headers().get("x-amzn-requestid")).map(str::to_string);
    let provider = match error {
        SdkError::TimeoutError(_) => ProviderError::timeout(message),
        SdkError::DispatchFailure(_) => ProviderError::network(message),
        SdkError::ResponseError(_) => ProviderError::server(message),
        SdkError::ServiceError(service) => match service.err().kind() {
            ExceptionKind::Throttled => ProviderError::rate_limit(message),
            ExceptionKind::Transient => ProviderError::server(message),
            ExceptionKind::Other => ProviderError::api(message),
        },
        _ => ProviderError::api(message),
    };
    provider.with_http_metadata(status, request_id).into()
}

impl BedrockException for ConverseStreamError {
    fn kind(&self) -> ExceptionKind {
        match self {
            Self::ThrottlingException(_) => ExceptionKind::Throttled,
            Self::ServiceUnavailableException(_)
            | Self::InternalServerException(_)
            | Self::ModelStreamErrorException(_)
            | Self::ModelTimeoutException(_)
            | Self::ModelNotReadyException(_) => ExceptionKind::Transient,
            _ => ExceptionKind::Other,
        }
    }
}

impl BedrockException for InvokeModelWithResponseStreamError {
    fn kind(&self) -> ExceptionKind {
        match self {
            Self::ThrottlingException(_) | Self::ServiceQuotaExceededException(_) => ExceptionKind::Throttled,
            Self::ServiceUnavailableException(_)
            | Self::InternalServerException(_)
            | Self::ModelStreamErrorException(_)
            | Self::ModelTimeoutException(_)
            | Self::ModelNotReadyException(_) => ExceptionKind::Transient,
            _ => ExceptionKind::Other,
        }
    }
}

impl BedrockException for ConverseStreamOutputError {
    fn kind(&self) -> ExceptionKind {
        match self {
            Self::ThrottlingException(_) => ExceptionKind::Throttled,
            Self::ServiceUnavailableException(_)
            | Self::InternalServerException(_)
            | Self::ModelStreamErrorException(_) => ExceptionKind::Transient,
            _ => ExceptionKind::Other,
        }
    }
}

impl BedrockException for ResponseStreamError {
    fn kind(&self) -> ExceptionKind {
        match self {
            Self::ThrottlingException(_) => ExceptionKind::Throttled,
            Self::ServiceUnavailableException(_)
            | Self::InternalServerException(_)
            | Self::ModelStreamErrorException(_)
            | Self::ModelTimeoutException(_) => ExceptionKind::Transient,
            _ => ExceptionKind::Other,
        }
    }
}

fn stream_error<T: BedrockException>(error: &SdkError<T, RawMessage>) -> LlmError {
    let message = format!("Bedrock stream error: {error}");
    let provider = match error {
        SdkError::ServiceError(service) => match service.err().kind() {
            ExceptionKind::Throttled => ProviderError::rate_limit(message),
            ExceptionKind::Transient => ProviderError::stream_interrupted(message),
            ExceptionKind::Other => ProviderError::api(message),
        },
        _ => ProviderError::stream_interrupted(message),
    };
    provider.into()
}

fn events<T, U>(receiver: EventReceiver<T, U>) -> impl Stream<Item = Result<T>> + Send
where
    T: Send,
    U: BedrockException + Send,
{
    stream::unfold(receiver, |mut receiver| async move {
        let event = receiver.recv().await.map_err(|e| {
            error!("Bedrock stream recv error: {e}");
            stream_error(&e)
        });
        event.transpose().map(|event| (event, receiver))
    })
}
