use aether_core::events::{TRACEPARENT_KEY, TRACESTATE_KEY, TraceContext};
use opentelemetry::Context;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum AgentTraceContext {
    /// Continue the trace beneath a remote W3C parent span.
    Parent(TraceContext),
    /// Start root spans in the supplied trace without attaching a parent span.
    Root {
        /// W3C trace ID: 32 lowercase hexadecimal characters and not all zeros.
        #[serde(rename = "traceId")]
        #[schemars(rename = "traceId")]
        trace_id: String,
    },
}

/// The trace context of `context`'s span, for propagation to another process.
/// `None` when the span is invalid and there is nothing to propagate.
pub(crate) fn inject_trace_context(context: &Context) -> Option<TraceContext> {
    if !context.span().span_context().is_valid() {
        return None;
    }

    let mut carrier = HashMap::new();
    TraceContextPropagator::new().inject_context(context, &mut carrier);
    Some(TraceContext { traceparent: carrier.remove(TRACEPARENT_KEY)?, tracestate: carrier.remove(TRACESTATE_KEY) })
}

/// The remote span `trace_context` names, for parenting local spans beneath it.
/// `None` when the traceparent does not name a valid span.
pub(crate) fn extract_trace_context(trace_context: &TraceContext) -> Option<Context> {
    let mut carrier = HashMap::from([(TRACEPARENT_KEY.to_string(), trace_context.traceparent.clone())]);
    carrier.extend(trace_context.tracestate.clone().map(|tracestate| (TRACESTATE_KEY.to_string(), tracestate)));
    let context = TraceContextPropagator::new().extract(&carrier);
    context.span().span_context().is_valid().then_some(context)
}
