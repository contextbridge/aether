use crate::Result;
use crate::providers::http::{SseData, open_sse, responses_code};
use crate::providers::response_stream::OpenedStream;
use reqwest::header::ACCEPT;
use reqwest::{Client, header::HeaderMap};

pub(crate) async fn send(
    http: &Client,
    url: &str,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Result<OpenedStream<SseData>> {
    open_sse(http.post(url).headers(headers).header(ACCEPT, "text/event-stream").json(&body), responses_code).await
}
