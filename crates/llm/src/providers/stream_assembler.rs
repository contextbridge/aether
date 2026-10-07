use std::collections::BTreeMap;
use std::fmt::Debug;

use futures::{Stream, StreamExt};
use tracing::warn;

use crate::{LlmResponse, ProviderError, Result, StopReason, ToolCallRequest};

pub(crate) fn assemble<T, U>(
    events: impl Stream<Item = Result<T>> + Send,
    mut decode: impl FnMut(T, &mut StreamAssembler<U>) -> Result<Vec<LlmResponse>> + Send,
) -> impl Stream<Item = Result<LlmResponse>> + Send
where
    T: Send,
    U: Ord + Copy + Debug + Send,
{
    async_stream::stream! {
        let mut events = std::pin::pin!(events);
        let mut turn = StreamAssembler::new();

        while let Some(event) = events.next().await {
            match event.and_then(|event| decode(event, &mut turn)) {
                Ok(responses) => {
                    for response in responses {
                        yield Ok(response);
                    }
                }
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }

        match turn.finish() {
            Ok(responses) => {
                for response in responses {
                    yield Ok(response);
                }
            }
            Err(error) => yield Err(error),
        }
    }
}

pub(crate) struct StreamAssembler<T> {
    tool_calls: BTreeMap<T, ToolCallRequest>,
    stop_reason: Option<StopReason>,
    terminated: bool,
}

impl<I: Ord + Copy + Debug> StreamAssembler<I> {
    pub fn new() -> Self {
        Self { tool_calls: BTreeMap::new(), stop_reason: None, terminated: false }
    }

    pub fn start_tool(&mut self, index: I, id: String, name: String) -> LlmResponse {
        let start = LlmResponse::ToolRequestStart { id: id.clone(), name: name.clone() };
        self.tool_calls.insert(index, ToolCallRequest { id, name, arguments: String::new() });
        start
    }

    pub fn append_tool_args(&mut self, index: I, chunk: String) -> Option<LlmResponse> {
        if chunk.is_empty() {
            return None;
        }

        let Some(tool_call) = self.tool_calls.get_mut(&index) else {
            warn!("Received tool call arguments for unknown index {index:?}");
            return None;
        };
        tool_call.arguments.push_str(&chunk);
        Some(LlmResponse::ToolRequestArg { id: tool_call.id.clone(), chunk })
    }

    pub fn complete_tool(&mut self, index: I) -> Option<LlmResponse> {
        self.tool_calls.remove(&index).map(|tool_call| LlmResponse::ToolRequestComplete { tool_call })
    }

    pub fn complete_tool_with(&mut self, index: I, mut tool_call: ToolCallRequest) -> Vec<LlmResponse> {
        let mut responses = Vec::new();
        match self.tool_calls.remove(&index) {
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

    pub fn terminate(&mut self) {
        self.terminated = true;
    }

    fn finish(mut self) -> Result<Vec<LlmResponse>> {
        if !self.terminated {
            return Err(ProviderError::stream_interrupted("stream ended before the provider's terminal event").into());
        }

        let mut responses = self.complete_all_tools();
        responses.push(LlmResponse::Done { stop_reason: self.stop_reason });
        Ok(responses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderErrorKind;

    #[test]
    fn test_tool_call_streams_and_completes() {
        let mut turn = StreamAssembler::<u32>::new();

        let start = turn.start_tool(0, "call_1".into(), "my_tool".into());
        let first = turn.append_tool_args(0, r#"{"key":"#.into());
        let second = turn.append_tool_args(0, r#""val"}"#.into());

        assert_eq!(start, LlmResponse::tool_request_start("call_1", "my_tool"));
        assert_eq!(first, Some(LlmResponse::tool_request_arg("call_1", r#"{"key":"#)));
        assert_eq!(second, Some(LlmResponse::tool_request_arg("call_1", r#""val"}"#)));
        assert_eq!(
            turn.complete_tool(0),
            Some(LlmResponse::tool_request_complete("call_1", "my_tool", r#"{"key":"val"}"#))
        );
    }

    #[test]
    fn test_complete_all_tools_orders_by_index() {
        let mut turn = StreamAssembler::<i32>::new();

        turn.start_tool(1, "b".into(), "tool_b".into());
        turn.start_tool(0, "a".into(), "tool_a".into());

        assert_eq!(
            turn.complete_all_tools(),
            vec![
                LlmResponse::tool_request_complete("a", "tool_a", ""),
                LlmResponse::tool_request_complete("b", "tool_b", "")
            ]
        );
        assert!(turn.complete_all_tools().is_empty());
    }

    #[test]
    fn test_empty_arguments_are_ignored() {
        let mut turn = StreamAssembler::<u32>::new();

        turn.start_tool(0, "id".into(), "tool".into());

        assert_eq!(turn.append_tool_args(0, String::new()), None);
    }

    #[test]
    fn test_arguments_for_unstarted_tool_call_are_ignored() {
        let mut turn = StreamAssembler::<u32>::new();

        assert_eq!(turn.append_tool_args(0, "{}".into()), None);
        assert!(turn.complete_all_tools().is_empty());
    }

    #[test]
    fn test_complete_tool_uses_only_that_tool_calls_arguments() {
        let mut turn = StreamAssembler::<u32>::new();

        turn.start_tool(0, "a".into(), "tool_a".into());
        turn.start_tool(1, "b".into(), "tool_b".into());
        turn.append_tool_args(0, r#"{"x":1}"#.into());

        assert_eq!(turn.complete_tool(0), Some(LlmResponse::tool_request_complete("a", "tool_a", r#"{"x":1}"#)));
        assert_eq!(turn.complete_all_tools(), vec![LlmResponse::tool_request_complete("b", "tool_b", "")]);
    }

    #[test]
    fn test_complete_unstarted_tool_returns_none() {
        let mut turn = StreamAssembler::<u32>::new();

        assert_eq!(turn.complete_tool(0), None);
    }

    #[test]
    fn test_complete_tool_with_supersedes_streamed_arguments() {
        let mut turn = StreamAssembler::<u32>::new();

        turn.start_tool(0, "a".into(), "tool_a".into());
        turn.append_tool_args(0, "{}".into());

        assert_eq!(
            turn.complete_tool_with(0, tool_call("a", "tool_a", r#"{"x":1}"#)),
            vec![LlmResponse::tool_request_complete("a", "tool_a", r#"{"x":1}"#)]
        );
    }

    #[test]
    fn test_complete_tool_with_empty_arguments_keeps_streamed_arguments() {
        let mut turn = StreamAssembler::<u32>::new();

        turn.start_tool(0, "a".into(), "tool_a".into());
        turn.append_tool_args(0, r#"{"x":1}"#.into());

        assert_eq!(
            turn.complete_tool_with(0, tool_call("a", "tool_a", "")),
            vec![LlmResponse::tool_request_complete("a", "tool_a", r#"{"x":1}"#)]
        );
    }

    #[test]
    fn test_complete_tool_with_starts_unstarted_tool_call() {
        let mut turn = StreamAssembler::<u32>::new();

        assert_eq!(
            turn.complete_tool_with(99, tool_call("a", "tool_a", "{}")),
            vec![
                LlmResponse::tool_request_start("a", "tool_a"),
                LlmResponse::tool_request_complete("a", "tool_a", "{}")
            ]
        );
    }

    #[tokio::test]
    async fn test_terminated_stream_completes_leftover_tools_then_done() {
        let responses =
            run(vec![Event::Start(0, "a"), Event::Args(0, "{}"), Event::Terminal(StopReason::ToolCalls)]).await;

        assert_eq!(
            responses.into_iter().collect::<Result<Vec<_>>>().unwrap(),
            vec![
                LlmResponse::tool_request_start("a", "tool"),
                LlmResponse::tool_request_arg("a", "{}"),
                LlmResponse::tool_request_complete("a", "tool", "{}"),
                LlmResponse::done_with_stop_reason(StopReason::ToolCalls),
            ]
        );
    }

    #[tokio::test]
    async fn test_stream_ending_before_terminal_event_is_interrupted() {
        let responses = run(vec![Event::Start(0, "a"), Event::Args(0, r#"{"x":"#)]).await;

        assert!(
            matches!(
                responses.as_slice(),
                [Ok(LlmResponse::ToolRequestStart { .. }), Ok(LlmResponse::ToolRequestArg { .. }), Err(error)]
                    if error.provider().map(|provider| provider.kind) == Some(ProviderErrorKind::StreamInterrupted)
            ),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn test_error_ends_the_stream() {
        let responses = run(vec![Event::Start(0, "a"), Event::Fail, Event::Terminal(StopReason::EndTurn)]).await;

        assert!(matches!(responses.as_slice(), [Ok(LlmResponse::ToolRequestStart { .. }), Err(_)]), "{responses:?}");
    }

    enum Event {
        Start(u32, &'static str),
        Args(u32, &'static str),
        Terminal(StopReason),
        Fail,
    }

    async fn run(events: Vec<Event>) -> Vec<Result<LlmResponse>> {
        assemble(tokio_stream::iter(events.into_iter().map(Ok)), decode).collect().await
    }

    fn decode(event: Event, turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
        Ok(match event {
            Event::Start(index, id) => vec![turn.start_tool(index, id.into(), "tool".into())],
            Event::Args(index, chunk) => turn.append_tool_args(index, chunk.into()).into_iter().collect(),
            Event::Terminal(stop_reason) => {
                turn.stop(stop_reason);
                turn.terminate();
                vec![]
            }
            Event::Fail => return Err(ProviderError::api("boom").into()),
        })
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest { id: id.to_string(), name: name.to_string(), arguments: arguments.to_string() }
    }
}
