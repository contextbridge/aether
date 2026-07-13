use crate::genai_constants as semconv;
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, KeyValue};

/// Owns one OpenTelemetry span; ends it as cancelled on drop.
pub(crate) struct SpanGuard {
    context: Context,
    cancel_message: &'static str,
    finished: bool,
}

impl SpanGuard {
    pub(crate) fn new(context: Context, cancel_message: &'static str) -> Self {
        Self { context, cancel_message, finished: false }
    }

    /// The context carrying the span, for parenting child spans.
    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn set_attribute(&self, attribute: KeyValue) {
        self.context.span().set_attribute(attribute);
    }

    pub(crate) fn add_event(&self, name: &'static str, attributes: Vec<KeyValue>) {
        self.context.span().add_event(name, attributes);
    }

    /// Ends the span with Ok status.
    pub(crate) fn end_ok(&mut self) {
        self.finished = true;
        let span = self.context.span();
        span.set_status(Status::Ok);
        span.end();
    }

    /// Ends the span with error status; `kind` is exported as `error.type`.
    pub(crate) fn end_error(&mut self, kind: Option<ErrorKind>, message: impl Into<String>) {
        self.finished = true;
        let span = self.context.span();
        if let Some(kind) = kind {
            span.set_attribute(KeyValue::new(semconv::ERROR_TYPE, kind.as_str()));
        }
        span.set_status(Status::error(message.into()));
        span.end();
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.end_error(Some(ErrorKind::Cancelled), self.cancel_message);
        }
    }
}

/// The closed set of `error.type` attribute values the observer emits.
#[derive(Clone, Copy)]
pub(crate) enum ErrorKind {
    Cancelled,
    LlmError,
    ToolError,
}

impl ErrorKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::LlmError => "llm_error",
            Self::ToolError => "tool_error",
        }
    }
}
