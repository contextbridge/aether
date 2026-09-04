use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::time::Duration;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, AgentObserver, LlmCallOutcome, StreamState, ToolEvent, TurnEvent, TurnOutcome};
use aether_core::testing::{AgentTrace, TestScenario, test_agent};
use aether_telemetry::ContentCaptureSettings;
use aether_telemetry::{
    AETHER_INPUT_MESSAGES_SHA256, AETHER_SYSTEM_INSTRUCTIONS_SHA256, GEN_AI_INPUT_MESSAGES, GEN_AI_OUTPUT_MESSAGES,
    GEN_AI_SYSTEM_INSTRUCTIONS, GEN_AI_TOOL_CALL_ARGUMENTS, GEN_AI_TOOL_CALL_RESULT, GEN_AI_TOOL_DEFINITIONS,
    GENAI_SEMCONV_SCHEMA_URL, GenAiMetrics, OtelInstrumentation, OtelObserver, genai_instrumentation_scope,
};
use common::{SYSTEM_INSTRUCTIONS_JSON, SYSTEM_PROMPT, SYSTEM_PROMPT_SHA256, all_content};

mod common;
use llm::testing::llm_response;
use llm::{LlmCallPurpose, LlmError, LlmResponse, ModelIdentity, ModelPricing, ProviderError, StopReason, TokenUsage};
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::{SpanKind, Status, TracerProvider as _};
use opentelemetry::{Array, Value};
use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use serde_json::Value as JsonValue;

#[tokio::test]
async fn tool_span_exposes_w3c_parent_context() -> Result<(), Box<dyn Error>> {
    let mut harness = otel_test().redacting().build();
    let observer = harness.observer.as_mut().expect("observer still alive");
    observer.on_event(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
    observer.on_event(&AgentEvent::Tool(ToolEvent::ExecutionStarted {
        tool_id: "call_1".to_string(),
        tool_name: "subagents__spawn_subagent".to_string(),
    }));
    let context = observer.tool_trace_context("call_1").expect("tool span exposes trace context");
    harness.drop_observer();
    let spans = harness.spans();
    let tool = spans.named("execute_tool subagents__spawn_subagent");
    let expected = format!("00-{}-{}-01", tool.span_context.trace_id(), tool.span_context.span_id());

    assert_eq!(context.traceparent, expected);
    Ok(())
}

#[tokio::test]
async fn spans_form_a_turn_rooted_hierarchy() -> Result<(), Box<dyn Error>> {
    let spans = otel_test().redacting().observe_trace(&happy_tool_trace().await?).spans();

    let turn = spans.named("invoke_agent");
    let tool = spans.named("execute_tool test__add_numbers");
    let chats = spans.prefixed("chat claude-sonnet-4-5");
    assert_eq!(chats.len(), 2, "one span per chat call: {:?}", spans.names());

    let turn_id = turn.span_context.span_id();
    let trace_id = turn.span_context.trace_id();
    assert_eq!(turn.status, Status::Ok);
    assert_eq!(tool.parent_span_id, turn_id, "tool span parents the turn");
    assert_eq!(tool.span_context.trace_id(), trace_id, "one trace per turn");
    assert_eq!(tool.status, Status::Ok);
    for chat in &chats {
        assert_eq!(chat.parent_span_id, turn_id, "chat span parents the turn");
        assert_eq!(chat.span_context.trace_id(), trace_id, "one trace per turn");
        assert_eq!(chat.status, Status::Ok);
        chat.assert_attr("gen_ai.provider.name", "anthropic");
        chat.assert_attr("gen_ai.request.model", "claude-sonnet-4-5");
        chat.assert_attr("aether.llm.attempt", 0);
    }
    chats[0].assert_attr("gen_ai.usage.input_tokens", 100);
    chats[0].assert_attr("gen_ai.usage.output_tokens", 20);
    chats[1].assert_attr("gen_ai.usage.input_tokens", 30);
    assert_eq!(tool.span_kind, SpanKind::Client);
    tool.assert_attr("mcp.method.name", "tools/call");
    tool.assert_attr("gen_ai.tool.name", "test__add_numbers");
    tool.assert_attr("mcp.tool.name", "add_numbers");
    tool.assert_attr("gen_ai.tool.call.id", "call_1");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failed_and_cancelled_calls_carry_error_attributes() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::from(ProviderError::server("boom".to_string()).with_http_status(503)))],
        vec![Ok(LlmResponse::start("m2")), Ok(LlmResponse::text("never seen")), Ok(LlmResponse::done())],
    ];
    let retry = RetryConfig { max_attempts: 5, base_delay: Duration::from_mins(1), max_delay: Duration::from_mins(1) };
    let trace = test_agent()
        .retry_config(retry)
        .llm_result_responses(&attempts)
        .scenario(TestScenario::new().user_text("go").wait_for_retry(1).cancel().wait_for_turn_end())
        .run_trace()
        .await?;

    let spans = otel_test().redacting().observe_trace(&trace).spans();

    let chats = spans.prefixed("chat");
    assert_eq!(chats.len(), 1, "the retry never starts after cancellation: {:?}", spans.names());

    let failed = chats[0];
    failed.assert_error_type("llm_error");
    assert!(
        matches!(&failed.status, Status::Error { description } if description.contains("boom")),
        "failed call carries the LLM error: {:?}",
        failed.status
    );

    let turn = spans.named("invoke_agent");
    turn.assert_error_type("cancelled");
    assert!(matches!(&turn.status, Status::Error { .. }));
    Ok(())
}

#[tokio::test]
async fn content_flags_gate_each_gen_ai_attribute() -> Result<(), Box<dyn Error>> {
    let trace = happy_tool_trace().await?;
    let input_messages = r#"[{"parts":[{"content":"3+5 = ?","type":"text"}],"role":"user"}]"#;
    let input_sha256 = "8d12ba59cd9df85123ca624d81a9a7c2f1015d3fb52318c2348227c5ac1ad8a7";

    let spans = otel_test().capturing().observe_trace_with_system_prompt(&trace).spans();
    let turn = spans.named("invoke_agent");
    turn.assert_attr(GEN_AI_INPUT_MESSAGES, input_messages);
    turn.assert_attr(
        GEN_AI_OUTPUT_MESSAGES,
        r#"[{"parts":[{"content":"hello The sum is 8","type":"text"}],"role":"assistant"}]"#,
    );
    let chat = spans.named("chat claude-sonnet-4-5");
    chat.assert_attr(GEN_AI_INPUT_MESSAGES, input_messages);
    chat.assert_attr(AETHER_INPUT_MESSAGES_SHA256, input_sha256);
    chat.assert_attr(
        GEN_AI_OUTPUT_MESSAGES,
        r#"[{"parts":[{"content":"hello ","type":"text"},{"arguments":{"a":3,"b":5},"id":"call_1","name":"test__add_numbers","type":"tool_call"}],"role":"assistant"}]"#,
    );
    let tool_definitions = chat.attr_string(GEN_AI_TOOL_DEFINITIONS).expect("tool definitions captured");
    let tool_definitions: JsonValue = serde_json::from_str(&tool_definitions)?;
    assert!(
        tool_definitions.as_array().is_some_and(|tools| tools.iter().any(|tool| {
            tool.get("type") == Some(&JsonValue::from("function"))
                && tool.get("name") == Some(&JsonValue::from("test__add_numbers"))
                && tool.get("parameters").is_some_and(JsonValue::is_object)
        })),
        "add_numbers definition missing from {tool_definitions}"
    );
    assert!(chat.attr("gen_ai.response.time_to_first_chunk").is_some());
    let tool = spans.named("execute_tool test__add_numbers");
    tool.assert_attr(GEN_AI_TOOL_CALL_ARGUMENTS, r#"{"a":3,"b":5}"#);
    tool.assert_attr(GEN_AI_TOOL_CALL_RESULT, "sum: 8");

    chat.assert_attr(GEN_AI_SYSTEM_INSTRUCTIONS, SYSTEM_INSTRUCTIONS_JSON);
    chat.assert_attr(AETHER_SYSTEM_INSTRUCTIONS_SHA256, SYSTEM_PROMPT_SHA256);

    let cases = [
        (ContentCaptureSettings::default(), &[][..]),
        (
            ContentCaptureSettings { system_instructions: true, ..Default::default() },
            &[GEN_AI_SYSTEM_INSTRUCTIONS, AETHER_SYSTEM_INSTRUCTIONS_SHA256],
        ),
        (
            ContentCaptureSettings { input_messages: true, ..ContentCaptureSettings::default() },
            &[GEN_AI_INPUT_MESSAGES, AETHER_INPUT_MESSAGES_SHA256],
        ),
        (
            ContentCaptureSettings { output_messages: true, ..ContentCaptureSettings::default() },
            &[GEN_AI_OUTPUT_MESSAGES],
        ),
        (
            ContentCaptureSettings { tool_definitions: true, ..ContentCaptureSettings::default() },
            &[GEN_AI_TOOL_DEFINITIONS],
        ),
        (
            ContentCaptureSettings { tool_calls: true, ..ContentCaptureSettings::default() },
            &[GEN_AI_TOOL_CALL_ARGUMENTS, GEN_AI_TOOL_CALL_RESULT],
        ),
    ];
    for (content, expected) in cases {
        let spans = otel_test().content(content).observe_trace_with_system_prompt(&trace).spans();
        assert_eq!(content_attribute_keys(&spans), expected.iter().copied().collect());
    }
    Ok(())
}

#[tokio::test]
async fn tool_only_llm_call_captures_generation_output() -> Result<(), Box<dyn Error>> {
    let request = serde_json::json!({ "a": 3, "b": 5 });
    let responses = [
        llm_response("m1")
            .tool_call("call_1", "test__add_numbers", &[&request.to_string()])
            .build_with_stop_reason(StopReason::ToolCalls),
        llm_response("m2").text(&["The sum is 8"]).build(),
    ];
    let trace = test_agent().llm_responses(&responses).user_text("3+5 = ?").run_trace().await?;

    let spans = otel_test().capturing().observe_trace(&trace).spans();

    let chat = spans.prefixed("chat")[0];
    chat.assert_attr(
        GEN_AI_OUTPUT_MESSAGES,
        r#"[{"finish_reason":"tool_call","parts":[{"arguments":{"a":3,"b":5},"id":"call_1","name":"test__add_numbers","type":"tool_call"}],"role":"assistant"}]"#,
    );
    chat.assert_attr("gen_ai.response.finish_reasons", finish_reasons("tool_call"));
    Ok(())
}

#[tokio::test]
async fn completed_llm_calls_capture_finish_reasons() -> Result<(), Box<dyn Error>> {
    let cases = [
        (StopReason::EndTurn, "stop"),
        (StopReason::Length, "length"),
        (StopReason::ToolCalls, "tool_call"),
        (StopReason::ContentFilter, "content_filter"),
        (StopReason::FunctionCall, "tool_call"),
        (StopReason::Unknown("provider_reason".to_string()), "provider_reason"),
    ];
    let mut events = vec![AgentEvent::Turn(TurnEvent::Started { content: vec![] })];
    for (index, (stop_reason, _)) in cases.iter().enumerate() {
        let outcome = LlmCallOutcome::Completed { stop_reason: Some(stop_reason.clone()), usage: None };
        events.extend(chat_call("test", &format!("model-{index}"), outcome));
    }
    events.push(AgentEvent::turn_ended(TurnOutcome::Completed));

    let spans = otel_test().redacting().observe_trace(&AgentTrace::from_events(events)).spans();

    for (index, (_, expected)) in cases.iter().enumerate() {
        spans
            .named(&format!("chat model-{index}"))
            .assert_attr("gen_ai.response.finish_reasons", finish_reasons(expected));
    }
    Ok(())
}

#[tokio::test]
async fn completed_llm_calls_emit_posthog_custom_pricing_and_token_properties() -> Result<(), Box<dyn Error>> {
    let usage = TokenUsage {
        cache_read_tokens: Some(40.into()),
        cache_creation_tokens: Some(10.into()),
        reasoning_tokens: Some(7.into()),
        ..TokenUsage::new(100, 20)
    };
    let events = AgentTrace::from_events(vec![
        AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            model: ModelIdentity {
                provider: Some("anthropic".to_string()),
                model_id: Some("priced-model".to_string()),
                pricing: Some(ModelPricing {
                    input_per_million: 3.0,
                    output_per_million: 15.0,
                    cache_read_per_million: Some(0.3),
                    cache_write_per_million: Some(3.75),
                }),
            },
            display_name: "priced-model".to_string(),
            attempt: 0,
            max_attempts: 1,
        }),
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: Some(usage) },
        }),
        AgentEvent::turn_ended(TurnOutcome::Completed),
    ]);

    let spans = otel_test().redacting().observe_trace(&events).spans();
    let chat = spans.named("chat priced-model");
    chat.assert_attr("$ai_input_token_price", 0.000_003);
    chat.assert_attr("$ai_output_token_price", 0.000_015);
    chat.assert_attr("$ai_cache_read_token_price", 0.000_000_3);
    chat.assert_attr("$ai_cache_write_token_price", 0.000_003_75);
    chat.assert_attr("$ai_reasoning_tokens", 7);
    // PostHog derives these from the semconv cache token attributes; duplicating
    // them would be redundant.
    chat.assert_no_attr("$ai_cache_read_input_tokens");
    chat.assert_no_attr("$ai_cache_creation_input_tokens");
    Ok(())
}

#[tokio::test]
async fn completed_llm_calls_capture_token_usage_breakdown() -> Result<(), Box<dyn Error>> {
    let usage = TokenUsage {
        cache_read_tokens: Some(40.into()),
        cache_creation_tokens: Some(10.into()),
        reasoning_tokens: Some(7.into()),
        ..TokenUsage::new(100, 20)
    };
    let mut events = vec![AgentEvent::Turn(TurnEvent::Started { content: vec![] })];
    events.extend(chat_call("test", "model", LlmCallOutcome::Completed { stop_reason: None, usage: Some(usage) }));
    events.push(AgentEvent::turn_ended(TurnOutcome::Completed));

    let telemetry = otel_test().redacting().observe_trace(&AgentTrace::from_events(events));

    let spans = telemetry.spans();
    let chat = spans.named("chat model");
    chat.assert_attr("gen_ai.usage.input_tokens", 100);
    chat.assert_attr("gen_ai.usage.output_tokens", 20);
    chat.assert_attr("gen_ai.usage.cache_read.input_tokens", 40);
    chat.assert_attr("gen_ai.usage.cache_creation.input_tokens", 10);
    chat.assert_attr("gen_ai.usage.reasoning.output_tokens", 7);

    let expected: BTreeSet<String> = ["input", "output"].into_iter().map(String::from).collect();
    assert_eq!(
        telemetry.metric_attribute_values("gen_ai.token.type"),
        expected,
        "the metric's token.type vocabulary is just input/output — breakdowns live on span attributes"
    );
    Ok(())
}

#[tokio::test]
async fn completed_message_sets_turn_output_without_streamed_chunks() -> Result<(), Box<dyn Error>> {
    let trace = AgentTrace::from_events(vec![
        AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
        AgentEvent::text("m1", "complete response", StreamState::Complete),
        AgentEvent::turn_ended(TurnOutcome::Completed),
    ]);

    let spans = otel_test().capturing().observe_trace(&trace).spans();
    spans.named("invoke_agent").assert_attr(
        GEN_AI_OUTPUT_MESSAGES,
        r#"[{"parts":[{"content":"complete response","type":"text"}],"role":"assistant"}]"#,
    );
    Ok(())
}

#[tokio::test]
async fn tool_error_ends_the_tool_span_with_error_status() -> Result<(), Box<dyn Error>> {
    let request = serde_json::json!({ "a": 8, "b": 0 });
    let responses = [
        llm_response("m1").tool_call("call_1", "test__divide_numbers", &[&request.to_string()]).build(),
        llm_response("m2").text(&["that did not work"]).build(),
    ];
    let trace = test_agent().llm_responses(&responses).user_text("8/0 = ?").run_trace().await?;

    let spans = otel_test().redacting().observe_trace(&trace).spans();

    let tool = spans.named("execute_tool test__divide_numbers");
    tool.assert_error_type("tool_error");
    assert!(
        matches!(&tool.status, Status::Error { description } if description.contains("Division by zero")),
        "tool span carries the tool error: {:?}",
        tool.status
    );
    Ok(())
}

#[tokio::test]
async fn compaction_call_is_tagged_and_parented_to_the_turn() -> Result<(), Box<dyn Error>> {
    let responses = [
        llm_response("m1").text(&["hi"]).usage(90_000, 10).build_with_stop_reason(StopReason::Length),
        llm_response("summary").text(&["summary"]).usage(50, 5).build(),
        llm_response("m2").text(&["done"]).build(),
    ];
    let trace =
        test_agent().context_window_override(100_000).llm_responses(&responses).user_text("go").run_trace().await?;

    let spans = otel_test().capturing().observe_trace(&trace).spans();

    let turn = spans.named("invoke_agent");
    let compaction = spans
        .iter()
        .find(|span| span.attr("aether.llm.purpose") == Some(Value::from("compaction")))
        .expect("compaction span tagged");
    assert_eq!(compaction.parent_span_id, turn.span_context.span_id());
    compaction.assert_attr("gen_ai.usage.input_tokens", 50);
    compaction.assert_no_attr(GEN_AI_INPUT_MESSAGES);
    compaction.assert_no_attr(GEN_AI_TOOL_DEFINITIONS);

    let chats: Vec<_> =
        spans.prefixed("chat").into_iter().filter(|span| span.attr("aether.llm.purpose").is_none()).collect();
    assert_eq!(chats.len(), 2, "each chat call gets its own span");
    assert!(
        chats.iter().all(|chat| chat.attr(GEN_AI_INPUT_MESSAGES).is_some()),
        "the turn input belongs to the chat calls, not the compaction call"
    );
    Ok(())
}

#[tokio::test]
async fn provider_names_map_to_genai_semconv() -> Result<(), Box<dyn Error>> {
    let completed = || LlmCallOutcome::Completed { stop_reason: None, usage: None };
    let mut events = vec![AgentEvent::Turn(TurnEvent::Started { content: vec![] })];
    events.extend(chat_call("gemini", "gemini-2.5-pro", completed()));
    events.extend(chat_call("my-custom-proxy", "custom-model", completed()));
    events.push(AgentEvent::turn_ended(TurnOutcome::Completed));

    let spans = otel_test().redacting().observe_trace(&AgentTrace::from_events(events)).spans();

    spans.named("chat gemini-2.5-pro").assert_attr("gen_ai.provider.name", "gcp.gemini");
    spans.named("chat custom-model").assert_attr("gen_ai.provider.name", "my-custom-proxy".to_string());
    Ok(())
}

#[tokio::test]
async fn dropping_the_observer_ends_open_spans_as_cancelled() -> Result<(), Box<dyn Error>> {
    let trace = happy_tool_trace().await?;
    let call_started = trace.position(|event| matches!(event, AgentEvent::Turn(TurnEvent::LlmCallStarted { .. })));

    let mut harness = otel_test().redacting().build();
    harness.feed(&trace.events()[..=call_started]);
    assert!(harness.spans().is_empty(), "nothing exported while spans are open");

    harness.drop_observer();
    let spans = harness.spans();
    let chat = spans.named("chat claude-sonnet-4-5");
    chat.assert_error_type("cancelled");
    let turn = spans.named("invoke_agent");
    turn.assert_error_type("cancelled");
    Ok(())
}

#[tokio::test]
async fn genai_metrics_use_the_expected_scope_units_and_attributes() -> Result<(), Box<dyn Error>> {
    let telemetry = otel_test().capturing().observe_trace(&happy_tool_trace().await?);
    let spans = telemetry.spans();
    let scope = &spans.named("invoke_agent").instrumentation_scope;
    assert_eq!(scope.name(), "aether.genai");
    assert_eq!(scope.version(), Some("test"));
    assert_eq!(scope.schema_url(), Some(GENAI_SEMCONV_SCHEMA_URL));

    let names = telemetry.metric_names();
    names.assert_contains_metric("gen_ai.client.operation.duration");
    names.assert_contains_metric("gen_ai.client.token.usage");
    names.assert_contains_metric("gen_ai.client.operation.time_to_first_chunk");

    let units = telemetry.metric_units();
    assert_eq!(units.get("gen_ai.client.operation.duration").map(String::as_str), Some("s"));
    assert_eq!(units.get("gen_ai.client.operation.time_to_first_chunk").map(String::as_str), Some("s"));
    assert_eq!(units.get("gen_ai.client.operation.time_per_output_chunk").map(String::as_str), Some("s"));
    assert_eq!(units.get("gen_ai.client.token.usage").map(String::as_str), Some("{token}"));

    let expected: BTreeSet<String> =
        ["gen_ai.operation.name", "gen_ai.provider.name", "gen_ai.request.model", "gen_ai.token.type"]
            .into_iter()
            .map(String::from)
            .collect();
    assert_eq!(
        telemetry.metric_attribute_keys(),
        expected,
        "metric attributes must stay a fixed low-cardinality set — no content, no per-call details"
    );
    Ok(())
}

/// A real agent turn that streams text, calls `test__add_numbers`, and reports
/// token usage, captured through the agent's actual event stream.
async fn happy_tool_trace() -> Result<AgentTrace, Box<dyn Error>> {
    let request = serde_json::json!({ "a": 3, "b": 5 });
    let responses = [
        llm_response("m1")
            .text(&["hello "])
            .tool_call("call_1", "test__add_numbers", &[&request.to_string()])
            .usage(100, 20)
            .build(),
        llm_response("m2").text(&["The sum is 8"]).usage(30, 7).build(),
    ];
    Ok(test_agent()
        .model("anthropic:claude-sonnet-4-5".parse()?)
        .system_prompt(aether_core::core::Prompt::Text(SYSTEM_PROMPT.to_string()))
        .llm_responses(&responses)
        .user_text("3+5 = ?")
        .run_trace()
        .await?)
}

const CONTENT_AND_HASH_ATTRIBUTES: [&str; 8] = [
    GEN_AI_SYSTEM_INSTRUCTIONS,
    GEN_AI_INPUT_MESSAGES,
    GEN_AI_OUTPUT_MESSAGES,
    GEN_AI_TOOL_DEFINITIONS,
    GEN_AI_TOOL_CALL_ARGUMENTS,
    GEN_AI_TOOL_CALL_RESULT,
    AETHER_SYSTEM_INSTRUCTIONS_SHA256,
    AETHER_INPUT_MESSAGES_SHA256,
];

fn content_attribute_keys(spans: &Spans) -> BTreeSet<&str> {
    spans
        .iter()
        .flat_map(|span| span.attributes.iter().map(|attribute| attribute.key.as_str()))
        .filter(|key| CONTENT_AND_HASH_ATTRIBUTES.contains(key))
        .collect()
}

fn chat_call(provider: &str, model: &str, outcome: LlmCallOutcome) -> [AgentEvent; 2] {
    [
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            model: ModelIdentity {
                provider: Some(provider.to_string()),
                model_id: Some(model.to_string()),
                pricing: None,
            },
            display_name: model.to_string(),
            attempt: 0,
            max_attempts: 1,
        }),
        AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Chat, outcome }),
    ]
}

fn finish_reasons(reason: &str) -> Value {
    Value::Array(Array::String(vec![reason.to_string().into()]))
}

fn otel_test() -> OtelTestBuilder {
    OtelTestBuilder::default()
}

#[derive(Default)]
struct OtelTestBuilder {
    content: ContentCaptureSettings,
}

struct OtelHarness {
    observer: Option<OtelObserver>,
    span_exporter: InMemorySpanExporter,
    metric_exporter: InMemoryMetricExporter,
    meter_provider: SdkMeterProvider,
    _tracer_provider: SdkTracerProvider,
}

impl OtelTestBuilder {
    fn capturing(self) -> Self {
        self.content(all_content())
    }

    fn redacting(self) -> Self {
        self.content(ContentCaptureSettings::default())
    }

    fn content(mut self, content: ContentCaptureSettings) -> Self {
        self.content = content;
        self
    }

    fn observe_trace(self, trace: &AgentTrace) -> OtelHarness {
        let mut harness = self.build();
        harness.feed(trace.events());
        harness
    }

    fn observe_trace_with_system_prompt(self, trace: &AgentTrace) -> OtelHarness {
        let mut harness = self.build();
        harness.observer.as_mut().expect("observer still alive").on_system_prompt(SYSTEM_PROMPT);
        harness.feed(trace.events());
        harness
    }

    fn build(self) -> OtelHarness {
        OtelHarness::new(self.content)
    }
}

impl OtelHarness {
    fn new(content: ContentCaptureSettings) -> Self {
        let span_exporter = InMemorySpanExporter::default();
        let scope = genai_instrumentation_scope("test");
        let tracer_provider = SdkTracerProvider::builder().with_simple_exporter(span_exporter.clone()).build();
        let metric_exporter = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder().with_periodic_exporter(metric_exporter.clone()).build();
        let metrics = GenAiMetrics::new(&meter_provider.meter_with_scope(scope.clone()));
        let observer = OtelObserver::new(OtelInstrumentation {
            tracer: tracer_provider.tracer_with_scope(scope),
            metrics,
            content,
            root_parent: None,
            agent_name: None,
        });
        Self {
            observer: Some(observer),
            span_exporter,
            metric_exporter,
            meter_provider,
            _tracer_provider: tracer_provider,
        }
    }

    fn feed(&mut self, events: &[AgentEvent]) {
        let observer = self.observer.as_mut().expect("observer still alive");
        for event in events {
            observer.on_event(event);
        }
    }

    fn drop_observer(&mut self) {
        self.observer = None;
    }

    fn spans(&self) -> Spans {
        Spans(self.span_exporter.get_finished_spans().expect("spans exported"))
    }

    fn finished_metrics(&self) -> Vec<ResourceMetrics> {
        self.meter_provider.force_flush().expect("metrics flushed");
        self.metric_exporter.get_finished_metrics().expect("metrics exported")
    }

    fn metric_names(&self) -> Vec<String> {
        self.finished_metrics()
            .iter()
            .flat_map(|resource| {
                resource.scope_metrics().flat_map(|scope| scope.metrics().map(|m| m.name().to_string()))
            })
            .collect::<Vec<_>>()
    }

    fn metric_units(&self) -> BTreeMap<String, String> {
        self.finished_metrics()
            .iter()
            .flat_map(|resource| resource.scope_metrics().flat_map(ScopeMetrics::metrics))
            .map(|metric| (metric.name().to_string(), metric.unit().to_string()))
            .collect()
    }

    fn metric_attributes(&self) -> Vec<(String, String)> {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

        fn histogram_attributes<T>(data: &MetricData<T>, pairs: &mut Vec<(String, String)>) {
            if let MetricData::Histogram(histogram) = data {
                for point in histogram.data_points() {
                    pairs.extend(point.attributes().map(|kv| (kv.key.to_string(), kv.value.to_string())));
                }
            }
        }

        let mut pairs = Vec::new();
        for resource in &self.finished_metrics() {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    match metric.data() {
                        AggregatedMetrics::F64(data) => histogram_attributes(data, &mut pairs),
                        AggregatedMetrics::U64(data) => histogram_attributes(data, &mut pairs),
                        AggregatedMetrics::I64(data) => histogram_attributes(data, &mut pairs),
                    }
                }
            }
        }
        pairs
    }

    fn metric_attribute_keys(&self) -> BTreeSet<String> {
        self.metric_attributes().into_iter().map(|(key, _)| key).collect()
    }

    fn metric_attribute_values(&self, key: &str) -> BTreeSet<String> {
        self.metric_attributes().into_iter().filter(|(k, _)| k == key).map(|(_, value)| value).collect()
    }
}

struct Spans(Vec<SpanData>);

impl Spans {
    fn named(&self, name: &str) -> &SpanData {
        self.iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("span {name:?} not found among {:?}", self.names()))
    }

    fn prefixed(&self, prefix: &str) -> Vec<&SpanData> {
        self.iter().filter(|span| span.name.starts_with(prefix)).collect()
    }

    fn names(&self) -> Vec<String> {
        self.iter().map(|span| span.name.to_string()).collect()
    }
}

impl std::ops::Deref for Spans {
    type Target = [SpanData];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

trait MetricNamesExt {
    fn assert_contains_metric(&self, name: &str);
}

impl MetricNamesExt for [String] {
    fn assert_contains_metric(&self, name: &str) {
        assert!(self.iter().any(|metric| metric == name), "metric {name:?} missing: {self:?}");
    }
}

trait SpanExt {
    fn attr(&self, key: &str) -> Option<Value>;
    fn attr_string(&self, key: &str) -> Option<String>;
    fn assert_attr(&self, key: &str, expected: impl Into<Value>);
    fn assert_no_attr(&self, key: &str);
    fn assert_error_type(&self, expected: &str);
}

impl SpanExt for SpanData {
    fn attr(&self, key: &str) -> Option<Value> {
        self.attributes.iter().find(|kv| kv.key.as_str() == key).map(|kv| kv.value.clone())
    }

    fn attr_string(&self, key: &str) -> Option<String> {
        match self.attr(key) {
            Some(Value::String(value)) => Some(value.to_string()),
            _ => None,
        }
    }

    fn assert_attr(&self, key: &str, expected: impl Into<Value>) {
        let actual = self.attr(key);
        let expected = expected.into();
        if let (Some(Value::String(actual_str)), Value::String(expected_str)) = (&actual, &expected)
            && let (Ok(actual_json), Ok(expected_json)) = (
                serde_json::from_str::<JsonValue>(actual_str.as_str()),
                serde_json::from_str::<JsonValue>(expected_str.as_str()),
            )
        {
            assert_eq!(actual_json, expected_json, "unexpected span attribute {key:?} on {:?}", self.name);
            return;
        }
        assert_eq!(actual, Some(expected), "unexpected span attribute {key:?} on {:?}", self.name);
    }

    fn assert_no_attr(&self, key: &str) {
        assert_eq!(self.attr(key), None, "unexpected span attribute {key:?} on {:?}", self.name);
    }

    fn assert_error_type(&self, expected: &str) {
        self.assert_attr("error.type", expected.to_string());
    }
}
