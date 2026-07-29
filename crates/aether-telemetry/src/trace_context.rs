use aether_core::events::{TRACEPARENT_KEY, TRACESTATE_KEY, TraceContext};
use opentelemetry::Context;
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

    let mut carrier = W3cCarrier::default();
    TraceContextPropagator::new().inject_context(context, &mut carrier);
    Some(TraceContext { traceparent: carrier.traceparent?, tracestate: carrier.tracestate })
}

/// The remote span `trace_context` names, for parenting local spans beneath it.
/// `None` when the traceparent does not name a valid span.
pub(crate) fn extract_trace_context(trace_context: &TraceContext) -> Option<Context> {
    let context = TraceContextPropagator::new().extract(&W3cCarrier::from(trace_context));
    context.span().span_context().is_valid().then_some(context)
}

/// Adapts [`TraceContext`] to the propagator's carrier API, which the orphan
/// rule keeps us from implementing on `TraceContext` itself.
#[derive(Default)]
struct W3cCarrier {
    traceparent: Option<String>,
    tracestate: Option<String>,
}

impl From<&TraceContext> for W3cCarrier {
    fn from(trace_context: &TraceContext) -> Self {
        Self { traceparent: Some(trace_context.traceparent.clone()), tracestate: trace_context.tracestate.clone() }
    }
}

impl Injector for W3cCarrier {
    fn set(&mut self, key: &str, value: String) {
        match key {
            TRACEPARENT_KEY => self.traceparent = Some(value),
            TRACESTATE_KEY => self.tracestate = Some(value),
            _ => {}
        }
    }
}

impl Extractor for W3cCarrier {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            TRACEPARENT_KEY => self.traceparent.as_deref(),
            TRACESTATE_KEY => self.tracestate.as_deref(),
            _ => None,
        }
    }

    fn keys(&self) -> Vec<&str> {
        [(TRACEPARENT_KEY, &self.traceparent), (TRACESTATE_KEY, &self.tracestate)]
            .into_iter()
            .filter_map(|(key, value)| value.is_some().then_some(key))
            .collect()
    }
}
