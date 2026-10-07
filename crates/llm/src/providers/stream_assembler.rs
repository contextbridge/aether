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
    U: Ord + Debug + Send,
{
    async_stream::try_stream! {
        let mut events = std::pin::pin!(events);
        let mut turn = StreamAssembler::new();

        while let Some(event) = events.next().await {
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

pub(crate) struct StreamAssembler<T> {
    tool_calls: BTreeMap<T, ToolCallRequest>,
    stop_reason: Option<StopReason>,
    termination: Termination,
}

impl<I: Ord + Debug> StreamAssembler<I> {
    pub fn new() -> Self {
        Self { tool_calls: BTreeMap::new(), stop_reason: None, termination: Termination::Pending }
    }

    pub fn start_tool(&mut self, index: I, id: String, name: String) -> LlmResponse {
        let start = LlmResponse::ToolRequestStart { id: id.clone(), name: name.clone() };
        self.tool_calls.insert(index, ToolCallRequest { id, name, arguments: String::new() });
        start
    }

    pub fn append_tool_args(&mut self, index: &I, chunk: String) -> Option<LlmResponse> {
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

    pub fn complete_tool(&mut self, index: &I) -> Option<LlmResponse> {
        self.tool_calls.remove(index).map(|tool_call| LlmResponse::ToolRequestComplete { tool_call })
    }

    pub fn complete_tool_with(&mut self, index: &I, mut tool_call: ToolCallRequest) -> Vec<LlmResponse> {
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
