// The upstream GenAI semantic conventions are still in development and every
// constant is marked deprecated until they're published in a stable crate.
#![allow(deprecated)]

use crate::observer::ErrorKind;
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, KeyValue};
use opentelemetry_semantic_conventions::attribute::{ERROR_TYPE, GEN_AI_TOOL_CALL_RESULT};

/// One tool call's streamed arguments and, once execution starts, its span.
/// Ends the span as cancelled on drop unless explicitly finished.
#[derive(Default)]
pub(crate) struct ToolCallState {
    pub(crate) arguments: String,
    pub(crate) span: Context,
    finished: bool,
}

impl ToolCallState {
    pub(crate) fn succeed(mut self, result: Option<String>) {
        self.finished = true;
        let span = self.span.span();
        if let Some(result) = result {
            span.set_attribute(KeyValue::new(GEN_AI_TOOL_CALL_RESULT, result));
        }
        span.set_status(Status::Ok);
        span.end();
    }

    pub(crate) fn fail(mut self, message: String) {
        self.finished = true;
        let span = self.span.span();
        span.set_attribute(KeyValue::new(ERROR_TYPE, ErrorKind::ToolError.as_str()));
        span.set_status(Status::error(message));
        span.end();
    }
}

impl Drop for ToolCallState {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let span = self.span.span();
        span.set_attribute(KeyValue::new(ERROR_TYPE, ErrorKind::Cancelled.as_str()));
        span.set_status(Status::error("turn ended before the tool completed"));
        span.end();
    }
}
