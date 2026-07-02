// The upstream GenAI semantic conventions are still in development and every
// constant is marked deprecated until they're published in a stable crate.
#![allow(deprecated)]

use opentelemetry_semantic_conventions::metric::{
    GEN_AI_CLIENT_OPERATION_DURATION, GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK,
    GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK, GEN_AI_CLIENT_TOKEN_USAGE,
};

#[derive(Clone)]
pub struct GenAiMetrics {
    pub(crate) duration: opentelemetry::metrics::Histogram<f64>,
    pub(crate) time_to_first_chunk: opentelemetry::metrics::Histogram<f64>,
    pub(crate) time_per_output_chunk: opentelemetry::metrics::Histogram<f64>,
    pub(crate) token_usage: opentelemetry::metrics::Histogram<u64>,
}

impl GenAiMetrics {
    pub fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        Self {
            duration: meter.f64_histogram(GEN_AI_CLIENT_OPERATION_DURATION).build(),
            time_to_first_chunk: meter.f64_histogram(GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK).build(),
            time_per_output_chunk: meter.f64_histogram(GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK).build(),
            token_usage: meter.u64_histogram(GEN_AI_CLIENT_TOKEN_USAGE).build(),
        }
    }
}
