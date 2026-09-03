use crate::content_capture::{ContentBuffer, ContentCaptureSettings};
use crate::content_json::{input_messages_json, output_messages_json, system_instructions_json, tool_definitions_json};
use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_constants as semconv;
use crate::hash::sha256_hex;
use crate::llm_call_state::LlmCallState;
use crate::span_guard::{ErrorKind, SpanGuard};
use crate::trace_context::inject_trace_context;
use aether_core::events::{
    AgentEvent, AgentObserver, MessageEvent, ToolEvent, TraceContext, TurnEvent, TurnOutcome, mcp_tool_name,
};
use llm::catalog::Provider;
use llm::{ContentBlock, LlmCallPurpose, ModelPricing, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use opentelemetry::trace::{SpanBuilder, SpanKind, TraceContextExt, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracer;
use std::collections::HashMap;

/// [`AgentObserver`] that renders an agent's event stream as OpenTelemetry
/// `GenAI` spans and metrics.
pub struct OtelObserver {
    turn: Option<TurnState>,
    tool_definitions: Vec<ToolDefinition>,
    system_prompt: Option<String>,
    otel: OtelInstrumentation,
}

/// The process-wide export pipeline — tracer, meters, and the capture and
/// parenting policy every span it produces inherits.
#[derive(Clone)]
pub struct OtelInstrumentation {
    pub tracer: SdkTracer,
    pub metrics: GenAiMetrics,
    pub content: ContentCaptureSettings,
    pub root_parent: Option<Context>,
    pub agent_name: Option<String>,
}

impl OtelObserver {
    pub fn new(otel: OtelInstrumentation) -> Self {
        Self { turn: None, tool_definitions: Vec::new(), system_prompt: None, otel }
    }
}

impl AgentObserver for OtelObserver {
    fn on_event(&mut self, message: &AgentEvent) {
        match message {
            AgentEvent::Turn(TurnEvent::Started { content }) => self.start_turn(content),
            AgentEvent::Turn(TurnEvent::Ended { outcome }) => {
                if let Some(turn) = self.turn.take() {
                    turn.finish(outcome);
                }
            }
            AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools }) => {
                self.tool_definitions.clone_from(tools);
            }
            message => {
                if let Some(turn) = &mut self.turn {
                    turn.on_event(message, &self.otel, &self.tool_definitions, self.system_prompt.as_deref());
                }
            }
        }
    }

    fn on_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = Some(prompt.to_string());
    }

    fn tool_trace_context(&self, tool_id: &str) -> Option<TraceContext> {
        let span = self.turn.as_ref()?.executing_tools.get(tool_id)?;
        inject_trace_context(span.context())
    }
}

impl OtelObserver {
    fn start_turn(&mut self, content: &[ContentBlock]) {
        // Drop (and thereby cancel) any stale turn and its open spans before
        // starting the new span, so the stale turn can't become its parent.
        self.turn = None;

        let input_text = ContentBlock::join_text(content);
        let mut input = ContentBuffer::new(self.otel.content.input_messages);
        input.set(&input_text);
        let operation_name = "invoke_agent";
        let mut attributes = vec![KeyValue::new(semconv::GEN_AI_OPERATION_NAME, operation_name)];
        let span_name = match &self.otel.agent_name {
            Some(agent_name) => {
                attributes.push(KeyValue::new(semconv::GEN_AI_AGENT_NAME, agent_name.clone()));
                format!("{operation_name} {agent_name}")
            }
            None => operation_name.to_string(),
        };
        if let Some(text) = input.get() {
            attributes.extend(hashed_content(
                semconv::GEN_AI_INPUT_MESSAGES,
                semconv::AETHER_INPUT_MESSAGES_SHA256,
                input_messages_json(text),
                text,
            ));
        }
        let builder = SpanBuilder::from_name(span_name).with_kind(SpanKind::Internal).with_attributes(attributes);
        let span_context = self.otel.start_span(builder, self.otel.root_parent.as_ref());
        let span = SpanGuard::new(span_context, TURN_CANCEL_MESSAGE);
        self.turn = Some(TurnState::new(span, input, self.otel.content.output_messages));
    }
}

impl OtelInstrumentation {
    /// Starts a span under `parent`. Providers for disabled tracing use an
    /// always-off sampler, so callers never need to branch.
    pub(crate) fn start_span(&self, builder: SpanBuilder, parent: Option<&Context>) -> Context {
        let parent = parent.cloned().unwrap_or_default();
        Context::new().with_span(self.tracer.build_with_context(builder, &parent))
    }
}

const TURN_CANCEL_MESSAGE: &str = "turn cancelled";
const TOOL_CANCEL_MESSAGE: &str = "turn ended before the tool completed";

/// All state scoped to one turn: the turn span plus any in-flight LLM-call and
/// tool spans and captured content. A turn exists by construction inside its
/// methods, and dropping it ends every still-open span as cancelled, so
/// replacing the value is all a reset takes.
struct TurnState {
    span: SpanGuard,
    input: ContentBuffer,
    output: ContentBuffer,
    chat_call: Option<LlmCallState>,
    compaction_call: Option<LlmCallState>,
    /// Tool-call arguments streamed before execution starts, keyed by call id.
    streamed_arguments: HashMap<String, ContentBuffer>,
    /// Spans of currently executing tools, keyed by call id.
    executing_tools: HashMap<String, SpanGuard>,
}

impl TurnState {
    fn new(span: SpanGuard, input: ContentBuffer, capture_output: bool) -> Self {
        Self {
            span,
            input,
            output: ContentBuffer::new(capture_output),
            chat_call: None,
            compaction_call: None,
            streamed_arguments: HashMap::new(),
            executing_tools: HashMap::new(),
        }
    }

    fn on_event(
        &mut self,
        message: &AgentEvent,
        instrumentation: &OtelInstrumentation,
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) {
        match message {
            AgentEvent::Turn(TurnEvent::LlmCallStarted { purpose, model, display_name, attempt, .. }) => {
                self.start_llm_call(
                    LlmCallStart {
                        purpose: *purpose,
                        provider: model.provider.as_deref(),
                        model: model.model_id.as_deref(),
                        display_name,
                        pricing: model.pricing,
                        attempt: *attempt,
                    },
                    instrumentation,
                    tools,
                    system_prompt,
                );
            }
            AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose, outcome }) => {
                if let Some(call) = self.llm_call_slot(*purpose).take() {
                    call.finish(outcome);
                }
            }
            AgentEvent::Message(
                MessageEvent::Text { message_id, chunk, is_complete: false }
                | MessageEvent::Thought { message_id, chunk, is_complete: false },
            ) => {
                if let Some(chat) = &mut self.chat_call {
                    chat.record_response_chunk(message_id, chunk);
                }
            }
            AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. }) => {
                self.output.push(chunk);
            }
            AgentEvent::Tool(ToolEvent::Call { request, .. }) => self.on_tool_call(request, instrumentation),
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) => {
                self.on_tool_call_update(tool_call_id, chunk);
            }
            AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, tool_name }) => {
                self.on_tool_execution_started(tool_id, tool_name, instrumentation);
            }
            AgentEvent::Tool(ToolEvent::Result { result, .. }) => self.on_tool_result(result, instrumentation),
            AgentEvent::Tool(ToolEvent::Error { error, .. }) => self.on_tool_error(error),
            _ => {}
        }
    }

    fn finish(self, outcome: &TurnOutcome) {
        let Self { mut span, output, chat_call, compaction_call, executing_tools, .. } = self;
        // Cancel any still-open call and tool spans before ending their parent.
        drop(chat_call);
        drop(compaction_call);
        drop(executing_tools);
        match outcome {
            TurnOutcome::Completed => {
                if let Some(text) = output_messages_json(output.get(), &[], None) {
                    span.set_attribute(KeyValue::new(semconv::GEN_AI_OUTPUT_MESSAGES, text));
                }
                span.end_ok();
            }
            TurnOutcome::Failed { error } => span.end_error(None, error.clone()),
            TurnOutcome::Cancelled => span.end_error(Some(ErrorKind::Cancelled), TURN_CANCEL_MESSAGE),
        }
    }

    fn start_llm_call(
        &mut self,
        call: LlmCallStart<'_>,
        instrumentation: &OtelInstrumentation,
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) {
        let model_name = call.model.unwrap_or(call.display_name).to_string();

        // Metrics backends key series by attribute values, so the metric set
        // stays a small fixed vocabulary — no content, no per-call details.
        let mut metric_attributes = vec![
            KeyValue::new(semconv::GEN_AI_OPERATION_NAME, "chat"),
            KeyValue::new(semconv::GEN_AI_REQUEST_MODEL, model_name.clone()),
        ];

        if let Some(provider) = call.provider {
            metric_attributes.push(KeyValue::new(semconv::GEN_AI_PROVIDER_NAME, genai_provider_name(provider)));
        }

        if call.purpose == LlmCallPurpose::Compaction {
            metric_attributes.push(KeyValue::new(semconv::LLM_PURPOSE, "compaction"));
        }

        let mut attributes = metric_attributes.clone();
        attributes.push(KeyValue::new(semconv::GEN_AI_REQUEST_STREAM, true));
        attributes.push(KeyValue::new(semconv::LLM_ATTEMPT, i64::from(call.attempt)));
        if let Some(pricing) = call.pricing {
            attributes.extend(pricing_attributes(pricing));
        }
        // Only chat calls carry the turn's input and tool definitions; a
        // compaction call's actual input is the internal summarization prompt.
        if call.purpose == LlmCallPurpose::Chat {
            if let Some(input) = self.input.get() {
                attributes.extend(hashed_content(
                    semconv::GEN_AI_INPUT_MESSAGES,
                    semconv::AETHER_INPUT_MESSAGES_SHA256,
                    input_messages_json(input),
                    input,
                ));
            }
            if instrumentation.content.tool_definitions && !tools.is_empty() {
                attributes.push(KeyValue::new(semconv::GEN_AI_TOOL_DEFINITIONS, tool_definitions_json(tools)));
            }
            if instrumentation.content.system_instructions
                && let Some(prompt) = system_prompt
            {
                attributes.extend(hashed_content(
                    semconv::GEN_AI_SYSTEM_INSTRUCTIONS,
                    semconv::AETHER_SYSTEM_INSTRUCTIONS_SHA256,
                    system_instructions_json(prompt),
                    prompt,
                ));
            }
        }

        let name = if model_name.is_empty() { "chat".to_string() } else { format!("chat {model_name}") };
        let builder = SpanBuilder::from_name(name).with_kind(SpanKind::Client).with_attributes(attributes);
        let context = instrumentation.start_span(builder, Some(self.span.context()));
        let state = LlmCallState::new(
            context,
            instrumentation.metrics.clone(),
            call.purpose,
            instrumentation.content.output_messages,
            metric_attributes,
        );
        *self.llm_call_slot(call.purpose) = Some(state);
    }

    fn on_tool_call(&mut self, request: &ToolCallRequest, instrumentation: &OtelInstrumentation) {
        if let Some(chat) = &mut self.chat_call {
            chat.record_tool_call_start(request);
        }
        let mut arguments = ContentBuffer::new(instrumentation.content.tool_calls);
        arguments.set(&request.arguments);
        self.streamed_arguments.insert(request.id.clone(), arguments);
    }

    fn on_tool_call_update(&mut self, tool_call_id: &str, chunk: &str) {
        if let Some(chat) = &mut self.chat_call {
            chat.record_tool_call_update(tool_call_id, chunk);
        }
        if let Some(arguments) = self.streamed_arguments.get_mut(tool_call_id) {
            arguments.push(chunk);
        }
    }

    fn on_tool_execution_started(&mut self, tool_id: &str, tool_name: &str, instrumentation: &OtelInstrumentation) {
        let mut attributes = vec![
            KeyValue::new(semconv::GEN_AI_OPERATION_NAME, "execute_tool"),
            KeyValue::new(semconv::GEN_AI_TOOL_NAME, tool_name.to_string()),
            KeyValue::new(semconv::GEN_AI_TOOL_CALL_ID, tool_id.to_string()),
            KeyValue::new(semconv::MCP_METHOD_NAME, "tools/call"),
            // The server sees the tool without our namespacing prefix, so this
            // is what joins this span to the server's span for the same call.
            KeyValue::new(semconv::MCP_TOOL_NAME, mcp_tool_name(tool_name).to_string()),
        ];
        let arguments = self.streamed_arguments.remove(tool_id);

        if let Some(text) = arguments.as_ref().and_then(ContentBuffer::get) {
            attributes.push(KeyValue::new(semconv::GEN_AI_TOOL_CALL_ARGUMENTS, text.to_string()));
        }

        let builder = SpanBuilder::from_name(format!("execute_tool {tool_name}"))
            .with_kind(SpanKind::Client)
            .with_attributes(attributes);
        let context = instrumentation.start_span(builder, Some(self.span.context()));
        self.executing_tools.insert(tool_id.to_string(), SpanGuard::new(context, TOOL_CANCEL_MESSAGE));
    }

    fn on_tool_result(&mut self, result: &ToolCallResult, instrumentation: &OtelInstrumentation) {
        self.streamed_arguments.remove(&result.id);
        let Some(mut span) = self.executing_tools.remove(&result.id) else { return };

        if instrumentation.content.tool_calls {
            span.set_attribute(KeyValue::new(semconv::GEN_AI_TOOL_CALL_RESULT, result.result.clone()));
        }

        span.end_ok();
    }

    fn on_tool_error(&mut self, error: &ToolCallError) {
        self.streamed_arguments.remove(&error.id);
        if let Some(mut span) = self.executing_tools.remove(&error.id) {
            span.end_error(Some(ErrorKind::ToolError), error.error.clone());
        }
    }

    fn llm_call_slot(&mut self, purpose: LlmCallPurpose) -> &mut Option<LlmCallState> {
        match purpose {
            LlmCallPurpose::Chat => &mut self.chat_call,
            LlmCallPurpose::Compaction => &mut self.compaction_call,
        }
    }
}

/// Maps the agent's canonical provider name to the `GenAI` semantic-convention
/// provider name, passing providers the catalog doesn't know through as-is.
fn genai_provider_name(provider: &str) -> String {
    provider.parse::<Provider>().map_or_else(|_| provider.to_string(), |p| p.genai_provider_name().to_string())
}

/// Borrowed view of [`TurnEvent::LlmCallStarted`]; named fields keep the two
/// optional strings from being swapped at the call site.
#[derive(Clone, Copy)]
struct LlmCallStart<'a> {
    purpose: LlmCallPurpose,
    provider: Option<&'a str>,
    model: Option<&'a str>,
    display_name: &'a str,
    pricing: Option<ModelPricing>,
    attempt: u32,
}

fn hashed_content(content_key: &'static str, hash_key: &'static str, content: String, text: &str) -> [KeyValue; 2] {
    [KeyValue::new(content_key, content), KeyValue::new(hash_key, sha256_hex(text))]
}

fn pricing_attributes(pricing: ModelPricing) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(semconv::AI_INPUT_TOKEN_PRICE, pricing.input_per_million / TOKENS_PER_MILLION),
        KeyValue::new(semconv::AI_OUTPUT_TOKEN_PRICE, pricing.output_per_million / TOKENS_PER_MILLION),
    ];

    if let Some(price) = pricing.cache_read_per_million {
        attributes.push(KeyValue::new(semconv::AI_CACHE_READ_TOKEN_PRICE, price / TOKENS_PER_MILLION));
    }

    if let Some(price) = pricing.cache_write_per_million {
        attributes.push(KeyValue::new(semconv::AI_CACHE_WRITE_TOKEN_PRICE, price / TOKENS_PER_MILLION));
    }

    attributes
}

const TOKENS_PER_MILLION: f64 = 1_000_000.0;
