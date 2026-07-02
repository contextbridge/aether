// This module makes heavy use of the upstream GenAI semantic conventions which
// are marked deprecated until they're published in a stable crate.
#![allow(deprecated)]

use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_semconv::{self as semconv, provider_registry_name};
use crate::llm_call_start::LlmCallStart;
use crate::llm_call_state::LlmCallState;
use crate::tool_call_state::ToolCallState;
use aether_core::events::{
    AgentEvent, AgentObserver, LlmCallOutcome, LlmCallPurpose, MessageEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use llm::{ContentBlock, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use opentelemetry::trace::{SpanBuilder, SpanKind, Status, TraceContextExt, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracer;
use opentelemetry_semantic_conventions::attribute::{
    ERROR_TYPE, GEN_AI_INPUT_MESSAGES, GEN_AI_OPERATION_NAME, GEN_AI_OUTPUT_MESSAGES, GEN_AI_PROVIDER_NAME,
    GEN_AI_REQUEST_MODEL, GEN_AI_REQUEST_STREAM, GEN_AI_SYSTEM, GEN_AI_TOOL_CALL_ARGUMENTS, GEN_AI_TOOL_CALL_ID,
    GEN_AI_TOOL_DEFINITIONS, GEN_AI_TOOL_NAME,
};
use std::collections::HashMap;
use std::time::Duration;

/// [`AgentObserver`] that renders an agent's event stream as OpenTelemetry
/// `GenAI` spans and metrics.
pub struct OtelObserver {
    tools: HashMap<String, ToolCallState>,
    llm_calls: HashMap<LlmCallPurpose, LlmCallState>,
    turn: Option<TurnGuard>,
    turn_input: String,
    turn_output: String,
    tool_definitions: Vec<ToolDefinition>,
    instrumentation: OtelInstrumentation,
}

#[derive(Clone)]
pub struct OtelInstrumentation {
    pub tracer: Option<SdkTracer>,
    pub metrics: Option<GenAiMetrics>,
    pub capture_content: bool,
}

impl OtelObserver {
    pub fn new(instrumentation: OtelInstrumentation) -> Self {
        Self {
            tools: HashMap::new(),
            llm_calls: HashMap::new(),
            turn: None,
            turn_input: String::new(),
            turn_output: String::new(),
            tool_definitions: Vec::new(),
            instrumentation,
        }
    }
}

impl AgentObserver for OtelObserver {
    fn on_event(&mut self, message: &AgentEvent) {
        match message {
            AgentEvent::Turn(TurnEvent::Started { content }) => self.on_turn_started(content),
            AgentEvent::Turn(TurnEvent::Ended { outcome }) => self.on_turn_ended(outcome),
            AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools }) => self.on_tool_definitions(tools),
            AgentEvent::Turn(TurnEvent::LlmCallStarted {
                purpose,
                provider,
                model,
                display_name,
                attempt,
                delay_ms,
                ..
            }) => {
                self.on_llm_call_started(LlmCallStart {
                    purpose: *purpose,
                    provider: provider.as_deref(),
                    model: model.as_deref(),
                    display_name,
                    attempt: *attempt,
                    delay: delay_ms.map(Duration::from_millis),
                });
            }
            AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose, outcome }) => self.on_llm_call_ended(*purpose, outcome),
            AgentEvent::Message(
                MessageEvent::Text { message_id, chunk, is_complete: false, .. }
                | MessageEvent::Thought { message_id, chunk, is_complete: false, .. },
            ) => {
                self.on_response_chunk(message_id, chunk);
            }
            AgentEvent::Tool(ToolEvent::Call { request, .. }) => self.on_tool_call(request),
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) => {
                self.on_tool_call_update(tool_call_id, chunk);
            }
            AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, tool_name }) => {
                self.on_tool_execution_started(tool_id, tool_name);
            }
            AgentEvent::Tool(ToolEvent::Result { result, .. }) => self.on_tool_result(result),
            AgentEvent::Tool(ToolEvent::Error { error, .. }) => self.on_tool_error(error),
            _ => {}
        }
    }
}

impl OtelObserver {
    fn on_turn_started(&mut self, content: &[ContentBlock]) {
        self.tools.clear();
        self.llm_calls.clear();
        self.turn_output.clear();
        self.turn_input =
            if self.instrumentation.capture_content { ContentBlock::join_text(content) } else { String::new() };
        // Drop (and thereby cancel) any stale turn before starting the new
        // span, so the stale turn can't become its parent.
        self.turn = None;
        let mut attributes = vec![KeyValue::new(GEN_AI_OPERATION_NAME, "invoke_agent")];
        if !self.turn_input.is_empty() {
            attributes.push(KeyValue::new(GEN_AI_INPUT_MESSAGES, input_messages_json(&self.turn_input)));
        }
        let builder = SpanBuilder::from_name("invoke_agent").with_kind(SpanKind::Internal).with_attributes(attributes);
        self.turn = Some(TurnGuard::new(self.start_span(builder)));
    }

    fn on_tool_definitions(&mut self, tools: &[ToolDefinition]) {
        self.tool_definitions.clear();
        self.tool_definitions.extend_from_slice(tools);
    }

    fn on_turn_ended(&mut self, outcome: &TurnOutcome) {
        self.tools.clear();
        self.llm_calls.clear();
        if let Some(turn) = self.turn.take() {
            turn.finish(outcome, self.instrumentation.capture_content.then(|| std::mem::take(&mut self.turn_output)));
        }
    }

    fn on_llm_call_started(&mut self, call: LlmCallStart<'_>) {
        let model_name = call.model.unwrap_or(call.display_name).to_string();
        let provider_name = call.provider.map(|provider| provider_registry_name(provider).to_string());

        // Metrics backends key series by attribute values, so the metric set
        // stays a small fixed vocabulary — no content, no per-call details.
        let mut metric_attributes =
            vec![KeyValue::new(GEN_AI_OPERATION_NAME, "chat"), KeyValue::new(GEN_AI_REQUEST_MODEL, model_name.clone())];
        if let Some(provider_name) = &provider_name {
            metric_attributes.push(KeyValue::new(GEN_AI_PROVIDER_NAME, provider_name.clone()));
        }
        if call.purpose == LlmCallPurpose::Compaction {
            metric_attributes.push(KeyValue::new(semconv::LLM_PURPOSE, "compaction"));
        }

        let mut attributes = metric_attributes.clone();
        attributes.push(KeyValue::new(GEN_AI_REQUEST_STREAM, true));
        attributes.push(KeyValue::new(semconv::LLM_ATTEMPT, i64::from(call.attempt)));
        if let Some(provider_name) = provider_name {
            attributes.push(KeyValue::new(GEN_AI_SYSTEM, provider_name));
        }
        if self.instrumentation.capture_content {
            if !self.turn_input.is_empty() {
                attributes.push(KeyValue::new(GEN_AI_INPUT_MESSAGES, input_messages_json(&self.turn_input)));
            }
            if !self.tool_definitions.is_empty() {
                attributes.push(KeyValue::new(GEN_AI_TOOL_DEFINITIONS, tool_definitions_json(&self.tool_definitions)));
            }
        }

        let name = if model_name.is_empty() { "chat".to_string() } else { format!("chat {model_name}") };
        let builder = SpanBuilder::from_name(name).with_kind(SpanKind::Client).with_attributes(attributes);
        let state = LlmCallState::new(
            self.start_span(builder),
            self.instrumentation.metrics.clone(),
            call.purpose,
            call.delay.unwrap_or_default(),
            self.instrumentation.capture_content,
            metric_attributes,
        );
        self.llm_calls.insert(call.purpose, state);
    }

    fn on_llm_call_ended(&mut self, purpose: LlmCallPurpose, outcome: &LlmCallOutcome) {
        if let Some(call) = self.llm_calls.remove(&purpose) {
            call.finish(outcome);
        }
    }

    fn on_response_chunk(&mut self, message_id: &str, chunk: &str) {
        let Some(chat) = self.llm_calls.get_mut(&LlmCallPurpose::Chat) else {
            return;
        };
        if self.instrumentation.capture_content {
            self.turn_output.push_str(chunk);
        }
        chat.record_response_chunk(message_id, chunk);
    }

    fn on_tool_call(&mut self, request: &ToolCallRequest) {
        if let Some(chat) = self.llm_calls.get_mut(&LlmCallPurpose::Chat) {
            chat.record_tool_call_start(&request.id, &request.name);
        }
        if self.instrumentation.capture_content {
            self.tools.entry(request.id.clone()).or_default().arguments.clone_from(&request.arguments);
        }
    }

    fn on_tool_call_update(&mut self, tool_call_id: &str, chunk: &str) {
        if let Some(chat) = self.llm_calls.get_mut(&LlmCallPurpose::Chat) {
            chat.record_output_chunk();
        }
        if self.instrumentation.capture_content
            && let Some(tool) = self.tools.get_mut(tool_call_id)
        {
            tool.arguments.push_str(chunk);
        }
    }

    fn on_tool_execution_started(&mut self, tool_id: &str, tool_name: &str) {
        let mut attributes = vec![
            KeyValue::new(GEN_AI_OPERATION_NAME, "execute_tool"),
            KeyValue::new(GEN_AI_TOOL_NAME, tool_name.to_string()),
            KeyValue::new(GEN_AI_TOOL_CALL_ID, tool_id.to_string()),
        ];
        if self.instrumentation.capture_content
            && let Some(tool) = self.tools.get(tool_id)
        {
            attributes.push(KeyValue::new(GEN_AI_TOOL_CALL_ARGUMENTS, tool.arguments.clone()));
        }
        let builder = SpanBuilder::from_name(format!("execute_tool {tool_name}"))
            .with_kind(SpanKind::Internal)
            .with_attributes(attributes);
        let span = self.start_span(builder);
        self.tools.entry(tool_id.to_string()).or_default().span = span;
    }

    fn on_tool_result(&mut self, result: &ToolCallResult) {
        if let Some(tool) = self.tools.remove(&result.id) {
            tool.succeed(self.instrumentation.capture_content.then(|| result.result.clone()));
        }
    }

    fn on_tool_error(&mut self, error: &ToolCallError) {
        if let Some(tool) = self.tools.remove(&error.id) {
            tool.fail(error.error.clone());
        }
    }

    /// Starts a span parented to the current turn. With tracing disabled this
    /// returns an empty [`Context`] whose span operations are all no-ops, so
    /// callers never need to branch.
    fn start_span(&self, builder: SpanBuilder) -> Context {
        let Some(tracer) = &self.instrumentation.tracer else {
            return Context::new();
        };
        let parent = self.turn.as_ref().map_or_else(Context::new, |turn| turn.context.clone());
        Context::new().with_span(tracer.build_with_context(builder, &parent))
    }
}

fn input_messages_json(input: &str) -> String {
    serde_json::json!([
        {
            "role": "user",
            "parts": [{ "type": "text", "content": input }]
        }
    ])
    .to_string()
}

pub(crate) fn output_messages_json(output: &str) -> String {
    serde_json::json!([
        {
            "role": "assistant",
            "parts": [{ "type": "text", "content": output }]
        }
    ])
    .to_string()
}

fn tool_definitions_json(tools: &[ToolDefinition]) -> String {
    serde_json::Value::Array(
        tools
            .iter()
            .map(|tool| {
                let parameters = serde_json::from_str::<serde_json::Value>(&tool.parameters)
                    .unwrap_or_else(|_| serde_json::json!({}));
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": parameters,
                })
            })
            .collect(),
    )
    .to_string()
}

/// Owns the turn span; ends it as cancelled on drop unless explicitly finished.
struct TurnGuard {
    context: Context,
    finished: bool,
}

impl TurnGuard {
    fn new(context: Context) -> Self {
        Self { context, finished: false }
    }

    fn finish(mut self, outcome: &TurnOutcome, output: Option<String>) {
        self.finished = true;
        match outcome {
            TurnOutcome::Cancelled => self.cancel(),
            TurnOutcome::Completed => {
                let span = self.context.span();
                if let Some(output) = output.filter(|output| !output.is_empty()) {
                    span.set_attribute(KeyValue::new(GEN_AI_OUTPUT_MESSAGES, output_messages_json(&output)));
                }
                span.set_status(Status::Ok);
                span.end();
            }
            TurnOutcome::Failed { error } => {
                let span = self.context.span();
                span.set_status(Status::error(error.clone()));
                span.end();
            }
        }
    }

    fn cancel(&self) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(ERROR_TYPE, ErrorKind::Cancelled.as_str()));
        span.set_status(Status::error("turn cancelled"));
        span.end();
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.cancel();
        }
    }
}

/// The closed set of `error.type` attribute values the observer emits.
#[derive(Clone, Copy)]
pub(crate) enum ErrorKind {
    Cancelled,
    LlmError,
    ToolError,
}

impl ErrorKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::LlmError => "llm_error",
            Self::ToolError => "tool_error",
        }
    }
}
