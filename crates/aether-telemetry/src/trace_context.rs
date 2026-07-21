use opentelemetry::propagation::Extractor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum AgentTraceContext {
    /// Continue the trace beneath a remote W3C parent span.
    Parent {
        /// W3C traceparent header: version, trace ID, parent span ID, and trace flags.
        traceparent: String,
        /// Optional W3C tracestate header carrying vendor-specific trace state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tracestate: Option<String>,
    },
    /// Start root spans in the supplied trace without attaching a parent span.
    Root {
        /// W3C trace ID: 32 lowercase hexadecimal characters and not all zeros.
        #[serde(rename = "traceId")]
        #[schemars(rename = "traceId")]
        trace_id: String,
    },
}

impl Extractor for AgentTraceContext {
    fn get(&self, key: &str) -> Option<&str> {
        match (self, key) {
            (Self::Parent { traceparent, .. }, "traceparent") => Some(traceparent),
            (Self::Parent { tracestate, .. }, "tracestate") => tracestate.as_deref(),
            _ => None,
        }
    }

    fn keys(&self) -> Vec<&str> {
        match self {
            Self::Parent { tracestate: Some(_), .. } => vec!["traceparent", "tracestate"],
            Self::Parent { tracestate: None, .. } => vec!["traceparent"],
            Self::Root { .. } => Vec::new(),
        }
    }
}
