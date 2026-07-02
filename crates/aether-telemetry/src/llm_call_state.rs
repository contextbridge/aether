// This module makes heavy use of the upstream GenAI semantic conventions which
// are marked deprecated until they're published in a stable crate.
#![allow(deprecated)]

use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_semconv as semconv;
use crate::observer::{ErrorKind, output_messages_json};
use aether_core::events::{LlmCallOutcome, LlmCallPurpose};
use llm::TokenUsage;
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, KeyValue};
use opentelemetry_semantic_conventions::attribute::{
    ERROR_TYPE, EXCEPTION_MESSAGE, EXCEPTION_TYPE, GEN_AI_OUTPUT_MESSAGES, GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK,
    GEN_AI_TOKEN_TYPE, GEN_AI_USAGE_INPUT_TOKENS, GEN_AI_USAGE_OUTPUT_TOKENS,
};
use std::time::{Duration, Instant};

/// One LLM call's span, metrics context, and streaming state. Ends as
/// cancelled on drop unless explicitly finished.
pub(crate) struct LlmCallState {
    span: Context,
    metrics: Option<GenAiMetrics>,
    start: Instant,
    /// Retry backoff scheduled before the request fired; subtracted from
    /// duration and time-to-first-chunk so metrics measure the call itself.
    delay: Duration,
    chunk_timing: ChunkTiming,
    response_started: bool,
    capture_content: bool,
    output: String,
    /// Fixed low-cardinality attribute set for metric samples; span
    /// attributes (content, attempt) must never leak into metrics.
    metric_attributes: Vec<KeyValue>,
    finished: bool,
}

impl LlmCallState {
    pub(crate) fn new(
        span: Context,
        metrics: Option<GenAiMetrics>,
        purpose: LlmCallPurpose,
        delay: Duration,
        capture_content: bool,
        metric_attributes: Vec<KeyValue>,
    ) -> Self {
        Self {
            span,
            metrics,
            start: Instant::now(),
            delay,
            chunk_timing: ChunkTiming::for_purpose(purpose),
            response_started: false,
            capture_content,
            output: String::new(),
            metric_attributes,
            finished: false,
        }
    }

    pub(crate) fn record_response_chunk(&mut self, message_id: &str, chunk: &str) {
        self.record_response_start(message_id);
        self.record_output_chunk();
        if self.capture_content {
            self.output.push_str(chunk);
        }
    }

    pub(crate) fn record_tool_call_start(&mut self, tool_call_id: &str, tool_call_name: &str) {
        self.record_output_chunk();
        let elapsed = self.elapsed_since_request().as_secs_f64();
        self.span.span().set_attribute(KeyValue::new(GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK, elapsed));
        self.span.span().add_event(
            semconv::GENAI_TOOL_CALL_START_EVENT,
            vec![
                KeyValue::new(semconv::TOOL_CALL_ID, tool_call_id.to_string()),
                KeyValue::new(semconv::TOOL_CALL_NAME, tool_call_name.to_string()),
            ],
        );
    }

    pub(crate) fn record_output_chunk(&mut self) {
        let now = Instant::now();
        match self.chunk_timing {
            ChunkTiming::Disabled => return,
            ChunkTiming::AwaitingFirst => {
                if let Some(metrics) = &self.metrics {
                    metrics
                        .time_to_first_chunk
                        .record(self.elapsed_since_request().as_secs_f64(), &self.metric_attributes);
                }
            }
            ChunkTiming::Streaming { last_chunk } => {
                if let Some(metrics) = &self.metrics {
                    metrics
                        .time_per_output_chunk
                        .record(now.duration_since(last_chunk).as_secs_f64(), &self.metric_attributes);
                }
            }
        }
        self.chunk_timing = ChunkTiming::Streaming { last_chunk: now };
    }

    pub(crate) fn finish(mut self, outcome: &LlmCallOutcome) {
        self.finished = true;
        match outcome {
            LlmCallOutcome::Completed { usage, .. } => {
                if self.capture_content && !self.output.is_empty() {
                    let output = std::mem::take(&mut self.output);
                    self.span
                        .span()
                        .set_attribute(KeyValue::new(GEN_AI_OUTPUT_MESSAGES, output_messages_json(&output)));
                }
                if let Some(usage) = usage {
                    let span = self.span.span();
                    span.set_attribute(KeyValue::new(GEN_AI_USAGE_INPUT_TOKENS, i64::from(usage.input_tokens)));
                    span.set_attribute(KeyValue::new(GEN_AI_USAGE_OUTPUT_TOKENS, i64::from(usage.output_tokens)));
                    self.record_token_usage(*usage);
                }
                self.span.span().set_status(Status::Ok);
                self.record_duration(None);
            }
            LlmCallOutcome::Failed { error, .. } => self.fail(ErrorKind::LlmError, error.clone()),
            LlmCallOutcome::Cancelled => self.fail(ErrorKind::Cancelled, "llm call cancelled".to_string()),
        }
        self.span.span().end();
    }

    fn record_response_start(&mut self, message_id: &str) {
        if self.response_started {
            return;
        }
        self.response_started = true;
        self.span.span().add_event(
            semconv::GENAI_RESPONSE_START_EVENT,
            vec![KeyValue::new(semconv::MESSAGE_ID, message_id.to_string())],
        );
    }

    fn fail(&mut self, kind: ErrorKind, message: String) {
        let span = self.span.span();
        span.add_event(
            semconv::EXCEPTION_EVENT,
            vec![KeyValue::new(EXCEPTION_TYPE, kind.as_str()), KeyValue::new(EXCEPTION_MESSAGE, message.clone())],
        );
        span.set_attribute(KeyValue::new(ERROR_TYPE, kind.as_str()));
        span.set_status(Status::error(message));
        self.record_duration(Some(kind));
    }

    fn elapsed_since_request(&self) -> Duration {
        self.start.elapsed().saturating_sub(self.delay)
    }

    fn record_duration(&self, error: Option<ErrorKind>) {
        let Some(metrics) = &self.metrics else { return };
        metrics.duration.record(self.elapsed_since_request().as_secs_f64(), &self.metric_attributes(error));
    }

    fn record_token_usage(&self, usage: TokenUsage) {
        let Some(metrics) = &self.metrics else { return };
        metrics.token_usage.record(u64::from(usage.input_tokens), &self.token_attributes("input"));
        metrics.token_usage.record(u64::from(usage.output_tokens), &self.token_attributes("output"));
        if let Some(tokens) = usage.cache_read_tokens {
            metrics.token_usage.record(u64::from(tokens), &self.token_attributes("input_cache_read"));
        }
        if let Some(tokens) = usage.reasoning_tokens {
            metrics.token_usage.record(u64::from(tokens), &self.token_attributes("output_reasoning"));
        }
    }

    fn metric_attributes(&self, error: Option<ErrorKind>) -> Vec<KeyValue> {
        let mut attributes = self.metric_attributes.clone();
        if let Some(error) = error {
            attributes.push(KeyValue::new(ERROR_TYPE, error.as_str()));
        }
        attributes
    }

    fn token_attributes(&self, token_type: &'static str) -> Vec<KeyValue> {
        let mut attributes = self.metric_attributes(None);
        attributes.push(KeyValue::new(GEN_AI_TOKEN_TYPE, token_type));
        attributes
    }
}

impl Drop for LlmCallState {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.fail(ErrorKind::Cancelled, "llm call cancelled".to_string());
        self.span.span().end();
    }
}

/// Streaming-chunk timing state for one LLM call; only chat calls report
/// chunk-timing metrics.
enum ChunkTiming {
    Disabled,
    AwaitingFirst,
    Streaming { last_chunk: Instant },
}

impl ChunkTiming {
    fn for_purpose(purpose: LlmCallPurpose) -> Self {
        match purpose {
            LlmCallPurpose::Chat => Self::AwaitingFirst,
            LlmCallPurpose::Compaction => Self::Disabled,
        }
    }
}
