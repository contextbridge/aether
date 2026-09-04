use std::pin::Pin;

use super::streaming::{ResponsesStreamEvent, process_response_stream};
use crate::providers::http::{HttpResponseMetadata, rejected, responses_code};
use crate::{LlmResponse, ProviderError, Result};
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use reqwest::header::ACCEPT;
use reqwest::{Client, header::HeaderMap};
use tracing::debug;

pub(crate) type ResponsesEventStream = Pin<Box<dyn Stream<Item = Result<ResponsesStreamEvent>> + Send>>;

pub(crate) struct ResponsesConnection {
    pub(crate) events: ResponsesEventStream,
    pub(crate) metadata: HttpResponseMetadata,
}

pub(crate) async fn send(
    http: &Client,
    url: &str,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Result<ResponsesConnection> {
    let response = http.post(url).headers(headers).header(ACCEPT, "text/event-stream").json(&body).send().await?;
    open_connection(response).await
}

pub(crate) async fn open_connection(response: reqwest::Response) -> Result<ResponsesConnection> {
    if !response.status().is_success() {
        return Err(rejected("Responses API", response, responses_code).await);
    }
    let metadata = HttpResponseMetadata::from(&response);
    Ok(ResponsesConnection { events: decode_response_sse(response), metadata })
}

pub(crate) fn process_connection(
    connection: ResponsesConnection,
) -> Pin<Box<dyn Stream<Item = Result<LlmResponse>> + Send>> {
    let ResponsesConnection { events, metadata } = connection;
    Box::pin(process_response_stream(events).map(move |result| {
        result.map_err(|error| match error.provider().cloned() {
            Some(provider) => provider.with_request_id(metadata.request_id.clone()).into(),
            None => error,
        })
    }))
}

pub(crate) fn decode_response_sse(response: reqwest::Response) -> ResponsesEventStream {
    Box::pin(response.bytes_stream().eventsource().filter_map(|result| {
        std::future::ready(match result {
            Ok(event) if event.data == "[DONE]" => None,
            Ok(event) => Some(serde_json::from_str::<ResponsesStreamEvent>(&event.data).map_err(|error| {
                debug!(data = event.data, %error, "Failed to decode Responses SSE event");
                ProviderError::stream_interrupted(format!("Invalid Responses SSE event: {error}")).into()
            })),
            Err(error) => Some(Err(ProviderError::stream_interrupted(error.to_string()).into())),
        })
    }))
}
