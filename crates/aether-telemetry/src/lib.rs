mod content_capture;
mod content_json;
pub mod error;
mod gen_ai_metrics;
mod genai_constants;
mod hash;
mod llm_call_state;
mod otel_observer;
mod span_guard;
mod telemetry_runtime;
mod trace_context;

pub use content_capture::ContentCaptureSettings;
pub use error::{TelemetryInitError, TelemetryShutdownError};
pub use gen_ai_metrics::GenAiMetrics;
pub use genai_constants::{
    AETHER_INPUT_MESSAGES_SHA256, AETHER_SYSTEM_INSTRUCTIONS_SHA256, GEN_AI_INPUT_MESSAGES, GEN_AI_OUTPUT_MESSAGES,
    GEN_AI_SYSTEM_INSTRUCTIONS, GEN_AI_TOOL_CALL_ARGUMENTS, GEN_AI_TOOL_CALL_RESULT, GEN_AI_TOOL_DEFINITIONS,
    GENAI_SEMCONV_SCHEMA_URL, genai_instrumentation_scope,
};
pub use otel_observer::{OtelInstrumentation, OtelObserver};
pub use telemetry_runtime::{TelemetryConfig, TelemetryRuntime};
pub use trace_context::AgentTraceContext;
