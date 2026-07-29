use rmcp::model::Meta;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Metadata key carrying the W3C traceparent, in MCP request metadata and HTTP headers alike.
pub const TRACEPARENT_KEY: &str = "traceparent";
/// Metadata key carrying the W3C tracestate.
pub const TRACESTATE_KEY: &str = "tracestate";

/// W3C trace context linking telemetry across process boundaries.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceContext {
    /// W3C traceparent header: version, trace ID, parent span ID, and trace flags.
    pub traceparent: String,
    /// Optional W3C tracestate header carrying vendor-specific trace state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
}

impl TraceContext {
    /// Reads the trace context a caller attached to MCP request metadata via
    /// [`to_meta`](Self::to_meta), if any.
    pub fn from_meta(meta: &Meta) -> Option<Self> {
        let traceparent = meta.0.get(TRACEPARENT_KEY)?.as_str()?.to_string();
        let tracestate = meta.0.get(TRACESTATE_KEY).and_then(|value| value.as_str()).map(str::to_string);
        Some(Self { traceparent, tracestate })
    }

    /// Attaches the trace context to MCP request metadata.
    pub fn to_meta(&self) -> Meta {
        let mut meta = Meta::new();
        meta.0.insert(TRACEPARENT_KEY.to_string(), self.traceparent.clone().into());
        if let Some(tracestate) = &self.tracestate {
            meta.0.insert(TRACESTATE_KEY.to_string(), tracestate.clone().into());
        }
        meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_round_trips_a_trace_context() {
        let trace_context = TraceContext {
            traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01".to_string(),
            tracestate: Some("vendor=value".to_string()),
        };

        assert_eq!(TraceContext::from_meta(&trace_context.to_meta()), Some(trace_context));
    }

    #[test]
    fn meta_round_trips_without_tracestate() {
        let trace_context = TraceContext {
            traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01".to_string(),
            tracestate: None,
        };

        let meta = trace_context.to_meta();

        assert_eq!(meta.0.len(), 1);
        assert_eq!(TraceContext::from_meta(&meta), Some(trace_context));
    }

    #[test]
    fn from_meta_returns_none_without_a_traceparent() {
        assert_eq!(TraceContext::from_meta(&Meta::new()), None);
    }
}
