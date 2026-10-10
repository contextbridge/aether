use crate::providers::http::HttpResponseMetadata;
use crate::{LlmError, LlmResponse, LlmResponseStream, ProviderError, Result, StopReason, ToolCallRequest};
use async_stream::{stream, try_stream};
use futures::{Stream, StreamExt};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::pin;
use std::time::Duration;
use tracing::warn;

pub(crate) fn response_stream<T, U, V>(
    open: impl Future<Output = Result<OpenedStream<T>>> + Send + 'static,
    decode: impl FnMut(U, &mut StreamAssembler<V>) -> Result<Vec<LlmResponse>> + Send + 'static,
    idle_timeout: Duration,
) -> LlmResponseStream
where
    T: Stream<Item = Result<U>> + Send + 'static,
    U: Send,
    V: Ord + Debug + Send + 'static,
{
    Box::pin(stream! {
        match within(idle_timeout, open).await.flatten() {
            Ok(OpenedStream { events, http }) => {
                let mut responses = pin!(decode_events(events, decode, idle_timeout));
                while let Some(response) = responses.next().await {
                    yield response.map_err(|error| match &http {
                        Some(http) => http.annotate(error),
                        None => error,
                    });
                }
            }
            Err(error) => yield Err(error),
        }
    })
}

pub(crate) fn error_stream(error: LlmError) -> LlmResponseStream {
    Box::pin(futures::stream::iter([Err(error)]))
}

pub(crate) struct OpenedStream<T> {
    events: T,
    http: Option<HttpResponseMetadata>,
}

impl<T> OpenedStream<T> {
    pub fn new(events: T) -> Self {
        Self { events, http: None }
    }

    pub fn with_http(events: T, http: HttpResponseMetadata) -> Self {
        Self { events, http: Some(http) }
    }
}

pub(crate) struct StreamAssembler<T> {
    tool_calls: BTreeMap<T, ToolCallRequest>,
    stop_reason: Option<StopReason>,
    termination: Termination,
}

impl<T: Ord + Debug> StreamAssembler<T> {
    pub fn new() -> Self {
        Self { tool_calls: BTreeMap::new(), stop_reason: None, termination: Termination::Pending }
    }

    pub fn start_tool(&mut self, index: T, id: String, name: String) -> LlmResponse {
        let start = LlmResponse::ToolRequestStart { id: id.clone(), name: name.clone() };
        self.tool_calls.insert(index, ToolCallRequest { id, name, arguments: String::new() });
        start
    }

    pub fn append_tool_args(&mut self, index: &T, chunk: String) -> Option<LlmResponse> {
        if chunk.is_empty() {
            return None;
        }

        let Some(tool_call) = self.tool_calls.get_mut(index) else {
            warn!("Received tool call arguments for unknown index {index:?}");
            return None;
        };
        tool_call.arguments.push_str(&chunk);
        Some(LlmResponse::ToolRequestArg { id: tool_call.id.clone(), chunk })
    }

    pub fn complete_tool(&mut self, index: &T) -> Option<LlmResponse> {
        self.tool_calls.remove(index).map(|tool_call| LlmResponse::ToolRequestComplete { tool_call })
    }

    pub fn complete_tool_with(&mut self, index: &T, mut tool_call: ToolCallRequest) -> Vec<LlmResponse> {
        let mut responses = Vec::new();
        match self.tool_calls.remove(index) {
            Some(streamed) if tool_call.arguments.is_empty() => tool_call.arguments = streamed.arguments,
            Some(_) => {}
            None => {
                responses
                    .push(LlmResponse::ToolRequestStart { id: tool_call.id.clone(), name: tool_call.name.clone() });
            }
        }
        responses.push(LlmResponse::ToolRequestComplete { tool_call });
        responses
    }

    pub fn complete_all_tools(&mut self) -> Vec<LlmResponse> {
        std::mem::take(&mut self.tool_calls)
            .into_values()
            .map(|tool_call| LlmResponse::ToolRequestComplete { tool_call })
            .collect()
    }

    pub fn stop(&mut self, stop_reason: StopReason) {
        self.stop_reason = Some(stop_reason);
    }

    pub fn allow_eof(&mut self) {
        self.termination = Termination::AtEof;
    }

    pub fn finish_now(&mut self) {
        self.termination = Termination::Immediate;
    }

    fn finish(mut self) -> Result<Vec<LlmResponse>> {
        if matches!(self.termination, Termination::Pending) {
            return Err(ProviderError::stream_interrupted("stream ended before the provider's terminal event").into());
        }

        let mut responses = self.complete_all_tools();
        responses.push(LlmResponse::Done { stop_reason: self.stop_reason });
        Ok(responses)
    }
}

enum Termination {
    Pending,
    AtEof,
    Immediate,
}

fn decode_events<T, U>(
    events: impl Stream<Item = Result<T>> + Send,
    mut decode: impl FnMut(T, &mut StreamAssembler<U>) -> Result<Vec<LlmResponse>> + Send,
    idle_timeout: Duration,
) -> impl Stream<Item = Result<LlmResponse>> + Send
where
    T: Send,
    U: Ord + Debug + Send,
{
    try_stream! {
        let mut events = pin!(events);
        let mut turn = StreamAssembler::new();
        yield LlmResponse::Start;

        while let Some(event) = within(idle_timeout, events.next()).await? {
            for response in decode(event?, &mut turn)? {
                yield response;
            }

            if matches!(turn.termination, Termination::Immediate) {
                break;
            }
        }

        for response in turn.finish()? {
            yield response;
        }
    }
}

async fn within<T>(idle_timeout: Duration, future: impl Future<Output = T>) -> Result<T> {
    tokio::time::timeout(idle_timeout, future)
        .await
        .map_err(|_| ProviderError::timeout(format!("Provider sent nothing for {}s", idle_timeout.as_secs())).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use std::future::{pending, ready};

    #[tokio::test(start_paused = true)]
    async fn provider_that_never_responds_times_out() {
        let responses = collect(text_stream(pending::<Result<OpenedStream<stream::Empty<_>>>>())).await;

        assert_eq!(responses.len(), 1, "{responses:?}");
        assert_timed_out(&responses);
    }

    #[tokio::test(start_paused = true)]
    async fn provider_that_goes_silent_mid_stream_times_out() {
        let events = stream::iter([Ok("partial")]).chain(stream::pending());

        let responses = collect(text_stream(ready(Ok(OpenedStream::new(events))))).await;

        assert!(matches!(&responses[..2], [Ok(LlmResponse::Start), Ok(LlmResponse::Text { .. })]), "{responses:?}");
        assert_timed_out(&responses);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_stream_that_keeps_sending_events_completes() {
        let events = stream::iter(["one", "two", "three"]).then(|text| async move {
            tokio::time::sleep(Duration::from_secs(50)).await;
            Ok(text)
        });

        let responses = collect(text_stream(ready(Ok(OpenedStream::new(events))))).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(matches!(responses.last(), Some(Ok(LlmResponse::Done { .. }))), "{responses:?}");
    }

    #[tokio::test]
    async fn errors_after_opening_carry_the_http_diagnostics() {
        let events = stream::iter([Err(ProviderError::stream_interrupted("connection reset").into())]);
        let http = HttpResponseMetadata { status: 200, request_id: Some("req-1".to_string()) };

        let responses = collect(text_stream(ready(Ok(OpenedStream::with_http(events, http))))).await;

        let error = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
        assert_eq!(error.and_then(|error| error.request_id.as_deref()), Some("req-1"), "{responses:?}");
        assert_eq!(error.and_then(|error| error.http_status), Some(200));
    }

    const IDLE_TIMEOUT: Duration = Duration::from_mins(1);

    fn text_stream<T>(open: impl Future<Output = Result<OpenedStream<T>>> + Send + 'static) -> LlmResponseStream
    where
        T: Stream<Item = Result<&'static str>> + Send + 'static,
    {
        let decode = |text: &'static str, turn: &mut StreamAssembler<u32>| {
            turn.allow_eof();
            Ok(vec![LlmResponse::Text { chunk: text.to_string() }])
        };
        response_stream(open, decode, IDLE_TIMEOUT)
    }

    async fn collect(responses: LlmResponseStream) -> Vec<Result<LlmResponse>> {
        responses.collect().await
    }

    fn assert_timed_out(responses: &[Result<LlmResponse>]) {
        let kind = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
        assert_eq!(kind.map(|error| error.kind), Some(crate::ProviderErrorKind::Timeout), "{responses:?}");
        assert!(!responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))), "{responses:?}");
    }
}
