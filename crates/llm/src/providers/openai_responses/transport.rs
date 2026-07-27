use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use tracing::debug;

use super::streaming::ResponsesStreamEvent;
use crate::{LlmError, Result};

pub(crate) type ResponsesEventStream = Pin<Box<dyn Stream<Item = Result<ResponsesStreamEvent>> + Send>>;

pub(crate) fn decode_response_sse(response: reqwest::Response) -> ResponsesEventStream {
    Box::pin(response.bytes_stream().eventsource().filter_map(|result| {
        std::future::ready(match result {
            Ok(event) if event.data == "[DONE]" => None,
            Ok(event) => Some(serde_json::from_str::<ResponsesStreamEvent>(&event.data).map_err(|error| {
                debug!(data = event.data, %error, "Failed to decode Responses SSE event");
                LlmError::StreamInterrupted(format!("Invalid Responses SSE event: {error}"))
            })),
            Err(error) => Some(Err(LlmError::StreamInterrupted(error.to_string()))),
        })
    }))
}
