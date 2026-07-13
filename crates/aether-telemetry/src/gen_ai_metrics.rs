use crate::genai_constants::{
    GEN_AI_CLIENT_OPERATION_DURATION, GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK,
    GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK, GEN_AI_CLIENT_TOKEN_USAGE,
};
use opentelemetry::metrics::{Histogram, Meter};

#[derive(Clone)]
pub struct GenAiMetrics {
    pub(crate) duration: Histogram<f64>,
    pub(crate) time_to_first_chunk: Histogram<f64>,
    pub(crate) time_per_output_chunk: Histogram<f64>,
    pub(crate) token_usage: Histogram<u64>,
}

impl GenAiMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram(GEN_AI_CLIENT_OPERATION_DURATION)
                .with_description("GenAI operation duration")
                .with_unit("s")
                .build(),

            time_to_first_chunk: meter
                .f64_histogram(GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK)
                .with_description("Time from request start until the first response chunk")
                .with_unit("s")
                .build(),

            time_per_output_chunk: meter
                .f64_histogram(GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK)
                .with_description("Elapsed time between consecutive output chunks")
                .with_unit("s")
                .build(),

            token_usage: meter
                .u64_histogram(GEN_AI_CLIENT_TOKEN_USAGE)
                .with_description("Number of input and output tokens used")
                .with_unit("{token}")
                .build(),
        }
    }
}
