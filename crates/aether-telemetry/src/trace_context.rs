use opentelemetry::propagation::Extractor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTraceContext {
    /// W3C traceparent header: version, trace ID, parent span ID, and trace flags.
    pub traceparent: String,
    /// Optional W3C tracestate header carrying vendor-specific trace state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
}

impl Extractor for AgentTraceContext {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            "traceparent" => Some(&self.traceparent),
            "tracestate" => self.tracestate.as_deref(),
            _ => None,
        }
    }

    fn keys(&self) -> Vec<&str> {
        let mut keys = vec!["traceparent"];
        if self.tracestate.is_some() {
            keys.push("tracestate");
        }
        keys
    }
}
