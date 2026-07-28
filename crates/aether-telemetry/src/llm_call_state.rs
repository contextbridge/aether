use crate::content_capture::{ContentBuffer, ContentCapture};
use crate::content_json::output_messages_json;
use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_constants::{
    AI_CACHE_REPORTING_EXCLUSIVE, AI_REASONING_TOKENS, ERROR_TYPE, GEN_AI_OUTPUT_MESSAGES,
    GEN_AI_RESPONSE_FINISH_REASONS, GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK, GEN_AI_TOKEN_TYPE,
    GEN_AI_USAGE_CACHE_CREATION_INPUT_TOKENS, GEN_AI_USAGE_CACHE_READ_INPUT_TOKENS, GEN_AI_USAGE_INPUT_TOKENS,
    GEN_AI_USAGE_OUTPUT_TOKENS, GEN_AI_USAGE_REASONING_OUTPUT_TOKENS, GENAI_RESPONSE_START_EVENT,
    GENAI_TOOL_CALL_START_EVENT, MESSAGE_ID, TOOL_CALL_ID, TOOL_CALL_NAME,
};
use crate::span_guard::{ErrorKind, SpanGuard};
use aether_core::events::{LlmCallOutcome, LlmCallPurpose};
use llm::{StopReason, TokenUsage, ToolCallRequest};
use opentelemetry::{Array, Context, KeyValue, Value};
use std::time::Instant;

/// One LLM call's span, metrics context, and streaming state. Ends as
/// cancelled on drop unless explicitly finished.
pub(crate) struct LlmCallState {
    span: SpanGuard,
    metrics: GenAiMetrics,
    start: Instant,
    chunk_timing: ChunkTiming,
    response_started: bool,
    output: ContentBuffer,
    tool_calls: Option<Vec<ToolCallRequest>>,
    metric_attributes: Vec<KeyValue>,
}

impl LlmCallState {
    pub(crate) fn new(
        context: Context,
        metrics: GenAiMetrics,
        purpose: LlmCallPurpose,
        capture: ContentCapture,
        metric_attributes: Vec<KeyValue>,
    ) -> Self {
        Self {
            span: SpanGuard::new(context, LLM_CANCEL_MESSAGE),
            metrics,
            start: Instant::now(),
            chunk_timing: ChunkTiming::for_purpose(purpose),
            response_started: false,
            output: capture.buffer(),
            tool_calls: capture.collect(),
            metric_attributes,
        }
    }

    pub(crate) fn record_response_chunk(&mut self, message_id: &str, chunk: &str) {
        self.record_response_start(message_id);
        self.record_output_chunk();
        self.output.push(chunk);
    }

    pub(crate) fn record_tool_call_start(&mut self, tool_call: &ToolCallRequest) {
        self.record_output_chunk();
        if let Some(tool_calls) = &mut self.tool_calls {
            tool_calls.push(tool_call.clone());
        }
        self.span.add_event(
            GENAI_TOOL_CALL_START_EVENT,
            vec![
                KeyValue::new(TOOL_CALL_ID, tool_call.id.clone()),
                KeyValue::new(TOOL_CALL_NAME, tool_call.name.clone()),
            ],
        );
    }

    pub(crate) fn record_tool_call_update(&mut self, tool_call_id: &str, chunk: &str) {
        self.record_output_chunk();
        if let Some(tool_call) = self
            .tool_calls
            .as_mut()
            .and_then(|tool_calls| tool_calls.iter_mut().find(|tool_call| tool_call.id == tool_call_id))
        {
            tool_call.arguments.push_str(chunk);
        }
    }

    pub(crate) fn record_output_chunk(&mut self) {
        let now = Instant::now();
        match self.chunk_timing {
            ChunkTiming::Disabled => return,
            ChunkTiming::AwaitingFirst => {
                let elapsed = self.start.elapsed().as_secs_f64();
                self.span.set_attribute(KeyValue::new(GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK, elapsed));
                self.metrics.time_to_first_chunk.record(elapsed, &self.metric_attributes);
            }
            ChunkTiming::Streaming { last_chunk } => {
                self.metrics
                    .time_per_output_chunk
                    .record(now.duration_since(last_chunk).as_secs_f64(), &self.metric_attributes);
            }
        }
        self.chunk_timing = ChunkTiming::Streaming { last_chunk: now };
    }

    pub(crate) fn finish(mut self, outcome: &LlmCallOutcome) {
        match outcome {
            LlmCallOutcome::Completed { stop_reason, usage } => {
                let finish_reason = stop_reason.as_ref().map(genai_finish_reason);
                if let Some(output) = output_messages_json(
                    self.output.get(),
                    self.tool_calls.as_deref().unwrap_or_default(),
                    finish_reason,
                ) {
                    self.span.set_attribute(KeyValue::new(GEN_AI_OUTPUT_MESSAGES, output));
                }
                if let Some(finish_reason) = finish_reason {
                    self.span.set_attribute(KeyValue::new(
                        GEN_AI_RESPONSE_FINISH_REASONS,
                        Value::Array(Array::String(vec![finish_reason.to_string().into()])),
                    ));
                }
                if let Some(usage) = usage {
                    self.span.set_attribute(KeyValue::new(GEN_AI_USAGE_INPUT_TOKENS, i64::from(usage.input_tokens)));
                    self.span.set_attribute(KeyValue::new(GEN_AI_USAGE_OUTPUT_TOKENS, i64::from(usage.output_tokens)));
                    if let Some(tokens) = usage.cache_read_tokens {
                        self.span.set_attribute(KeyValue::new(GEN_AI_USAGE_CACHE_READ_INPUT_TOKENS, i64::from(tokens)));
                    }
                    if let Some(tokens) = usage.cache_creation_tokens {
                        self.span
                            .set_attribute(KeyValue::new(GEN_AI_USAGE_CACHE_CREATION_INPUT_TOKENS, i64::from(tokens)));
                    }
                    if let Some(exclusive) = usage.cache_reporting_exclusive {
                        self.span.set_attribute(KeyValue::new(AI_CACHE_REPORTING_EXCLUSIVE, exclusive));
                    }
                    if let Some(tokens) = usage.reasoning_tokens {
                        let tokens = i64::from(tokens);
                        self.span.set_attribute(KeyValue::new(GEN_AI_USAGE_REASONING_OUTPUT_TOKENS, tokens));
                        self.span.set_attribute(KeyValue::new(AI_REASONING_TOKENS, tokens));
                    }
                    self.record_token_usage(*usage);
                }
                self.span.end_ok();
                self.record_duration(None);
            }
            LlmCallOutcome::Failed { error, .. } => self.fail(ErrorKind::LlmError, error.clone()),
            LlmCallOutcome::Cancelled => self.cancel(),
        }
    }

    fn record_response_start(&mut self, message_id: &str) {
        if self.response_started {
            return;
        }
        self.response_started = true;
        self.span.add_event(GENAI_RESPONSE_START_EVENT, vec![KeyValue::new(MESSAGE_ID, message_id.to_string())]);
    }

    fn cancel(&mut self) {
        self.fail(ErrorKind::Cancelled, LLM_CANCEL_MESSAGE.to_string());
    }

    fn fail(&mut self, kind: ErrorKind, message: String) {
        self.span.end_error(Some(kind), message);
        self.record_duration(Some(kind));
    }

    fn record_duration(&self, error: Option<ErrorKind>) {
        self.metrics.duration.record(self.start.elapsed().as_secs_f64(), &self.attributes_with_error(error));
    }

    fn record_token_usage(&self, usage: TokenUsage) {
        self.metrics.token_usage.record(u64::from(usage.input_tokens), &self.token_attributes("input"));
        self.metrics.token_usage.record(u64::from(usage.output_tokens), &self.token_attributes("output"));
    }

    fn attributes_with_error(&self, error: Option<ErrorKind>) -> Vec<KeyValue> {
        let mut attributes = self.metric_attributes.clone();
        if let Some(error) = error {
            attributes.push(KeyValue::new(ERROR_TYPE, error.as_str()));
        }
        attributes
    }

    fn token_attributes(&self, token_type: &'static str) -> Vec<KeyValue> {
        let mut attributes = self.attributes_with_error(None);
        attributes.push(KeyValue::new(GEN_AI_TOKEN_TYPE, token_type));
        attributes
    }
}

impl Drop for LlmCallState {
    fn drop(&mut self) {
        if !self.span.is_finished() {
            self.cancel();
        }
    }
}

fn genai_finish_reason(stop_reason: &StopReason) -> &str {
    match stop_reason {
        StopReason::EndTurn => "stop",
        StopReason::Length => "length",
        StopReason::ToolCalls | StopReason::FunctionCall => "tool_call",
        StopReason::ContentFilter => "content_filter",
        StopReason::Unknown(reason) => reason,
    }
}

const LLM_CANCEL_MESSAGE: &str = "llm call cancelled";

/// Streaming-chunk timing state for one LLM call; only chat calls report
/// chunk timing.
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
