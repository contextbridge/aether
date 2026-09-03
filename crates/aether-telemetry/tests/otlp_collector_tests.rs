use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aether_core::events::{AgentEvent, LlmCallOutcome, ToolEvent, TraceContext, TurnEvent, TurnOutcome};
use aether_telemetry::{
    AETHER_SYSTEM_INSTRUCTIONS_SHA256, AgentTraceContext, ContentCaptureSettings, GEN_AI_SYSTEM_INSTRUCTIONS,
    TelemetryConfig, TelemetryRuntime,
};
use common::{SYSTEM_INSTRUCTIONS_JSON, SYSTEM_PROMPT, SYSTEM_PROMPT_SHA256, all_content};

mod common;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use llm::{LlmCallPurpose, ModelIdentity, ModelPricing, TokenUsage};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::Span;
use prost::Message;

#[tokio::test(flavor = "multi_thread")]
async fn runtime_exports_metrics_to_a_signal_specific_endpoint() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        endpoint: None,
        metrics_endpoint: Some(collector.signal_specific_metrics_endpoint()),
        headers: test_headers("signal-specific"),
        traces_enabled: false,
        ..collector_config(&collector)
    })
    .expect("runtime initializes against a signal-specific endpoint");
    observe_a_turn(&runtime, None, None, None);

    runtime.shutdown().expect("runtime flushes metrics");
    let exports = collector.exports();

    assert!(exports.traces.is_empty());
    assert_eq!(exports.metrics.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_exports_traces_to_a_signal_specific_endpoint() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        endpoint: None,
        traces_endpoint: Some(collector.signal_specific_traces_endpoint()),
        headers: test_headers("signal-specific"),
        metrics_enabled: false,
        ..collector_config(&collector)
    })
    .expect("runtime initializes against a signal-specific endpoint");
    observe_a_turn(&runtime, None, None, None);

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();

    assert_eq!(exports.traces.len(), 1);
    assert!(exports.metrics.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_parents_every_turn_to_the_supplied_trace_context() {
    let collector = FakeOtlpCollector::start().await;
    let trace_context = serde_json::from_str::<AgentTraceContext>(
        r#"{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"vendor=value"}"#,
    )
    .expect("valid trace context");
    let expected_trace_id =
        vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
    let expected_parent_span_id = vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("fixed-trace"),
        sample_ratio: 0.0,
        trace_context: Some(trace_context),
        ..collector_config(&collector)
    })
    .expect("runtime initializes against collector");

    for _ in 0..2 {
        observe_a_turn(&runtime, None, None, None);
    }

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();
    let spans = trace_spans(&exports);

    assert_propagated_hierarchy(&spans, &expected_trace_id, &expected_parent_span_id);
    assert!(!exports.metrics.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn observer_factory_prefers_a_dynamic_parent_context() {
    let collector = FakeOtlpCollector::start().await;
    let parent = TraceContext {
        traceparent: "00-ffeeddccbbaa99887766554433221100-fedcba9876543210-01".to_string(),
        tracestate: Some("vendor=dynamic".to_string()),
    };
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("dynamic-parent"),
        sample_ratio: 0.0,
        ..collector_config(&collector)
    })
    .expect("runtime initializes against collector");
    observe_a_turn(&runtime, Some(&parent), None, None);

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();
    let spans = trace_spans(&exports);
    let turn = spans.iter().find(|span| span.name == "invoke_agent").expect("turn span exported");

    assert_eq!(
        turn.trace_id,
        vec![0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00]
    );
    assert_eq!(turn.parent_span_id, vec![0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10]);
}

#[tokio::test(flavor = "multi_thread")]
async fn observer_factory_names_agent_spans() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("named-agent"),
        ..collector_config(&collector)
    })
    .expect("runtime initializes against collector");
    {
        let mut observer = runtime.observer_factory().agent(Some("PatchWaveFix"), None);
        observer.on_event(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
        observer.on_event(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }));
    }

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();
    let spans = trace_spans(&exports);
    let turn = spans.iter().find(|span| span.name == "invoke_agent PatchWaveFix").expect("named agent span exported");

    assert_eq!(string_attribute(turn, "gen_ai.operation.name"), Some("invoke_agent"));
    assert_eq!(string_attribute(turn, "gen_ai.agent.name"), Some("PatchWaveFix"));
}

#[tokio::test(flavor = "multi_thread")]
async fn propagated_mcp_context_connects_parent_tool_server_and_child_agent_spans() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("mcp-hierarchy"),
        ..collector_config(&collector)
    })
    .expect("runtime initializes against collector");
    {
        let factory = runtime.observer_factory();
        let mut parent = factory.agent(Some("PatchWaveFix"), None);
        parent.on_event(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
        parent.on_event(&AgentEvent::Tool(ToolEvent::ExecutionStarted {
            tool_id: "call_1".to_string(),
            tool_name: "subagents__spawn_subagent".to_string(),
        }));
        let outbound = parent.tool_trace_context("call_1").expect("outbound tool span provides propagation context");

        let server = factory.tool_call_request("spawn_subagent", Some(&outbound));
        let mut child = factory.agent(Some("Explore"), server.trace_context().as_ref());
        child.on_event(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
        child.on_event(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }));
        server.finish(None);
        parent.on_event(&AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }));
    }

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();
    let spans = trace_spans(&exports);
    let tool = spans
        .iter()
        .find(|span| span.name == "execute_tool subagents__spawn_subagent")
        .expect("parent tool span exported");
    let server = spans.iter().find(|span| span.name == "tools/call spawn_subagent").expect("MCP server span exported");
    let child = spans
        .iter()
        .find(|span| span.name == "invoke_agent Explore" && span.parent_span_id == server.span_id)
        .expect("child agent span is parented by the MCP server span");

    assert_eq!(server.trace_id, tool.trace_id);
    assert_eq!(server.parent_span_id, tool.span_id);
    assert_eq!(child.trace_id, tool.trace_id);
    // Both ends of the call agree on the tool's wire name, so a backend can
    // group them even though the caller knows it by its namespaced name.
    assert_eq!(string_attribute(tool, "mcp.tool.name"), Some("spawn_subagent"));
    assert_eq!(string_attribute(server, "mcp.tool.name"), Some("spawn_subagent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_starts_root_spans_with_the_supplied_trace_id() {
    let collector = FakeOtlpCollector::start().await;
    let trace_context = serde_json::from_str::<AgentTraceContext>(r#"{"traceId":"00112233445566778899aabbccddeeff"}"#)
        .expect("valid trace ID context");
    let expected_trace_id =
        vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("fixed-trace-root"),
        trace_context: Some(trace_context),
        ..collector_config(&collector)
    })
    .expect("runtime initializes against collector");

    for _ in 0..2 {
        observe_a_turn(&runtime, None, None, None);
    }

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();
    let spans = trace_spans(&exports);

    assert_eq!(spans.len(), 4);
    assert!(spans.iter().all(|span| span.trace_id == expected_trace_id));
    let roots = spans.iter().filter(|span| span.name == "invoke_agent").copied().collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().all(|span| span.parent_span_id.is_empty()));
    for child in spans.iter().filter(|span| span.name != "invoke_agent") {
        assert!(roots.iter().any(|root| child.parent_span_id == root.span_id));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_exports_genai_spans_and_metrics_to_an_otlp_collector() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        headers: test_headers("collector-test"),
        ..capturing_collector_config(&collector)
    })
    .expect("runtime initializes against collector");
    observe_a_turn(&runtime, None, Some("PatchWaveFix"), Some(SYSTEM_PROMPT));

    runtime.shutdown().expect("runtime flushes both signals");
    let exports = collector.exports();
    let spans = trace_spans(&exports);
    let span_names = spans.iter().map(|span| span.name.as_str()).collect::<Vec<_>>();

    let metric_names = exports
        .metrics
        .iter()
        .flat_map(|request| &request.resource_metrics)
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .map(|metric| metric.name.as_str())
        .collect::<Vec<_>>();

    assert!(span_names.contains(&"invoke_agent PatchWaveFix"));
    assert!(span_names.contains(&"chat test-model"));
    let turn = spans.iter().find(|span| span.name == "invoke_agent PatchWaveFix").expect("turn span exported");
    let chat = spans.iter().find(|span| span.name == "chat test-model").expect("chat span exported");
    let attribute_keys = chat.attributes.iter().map(|attribute| attribute.key.as_str()).collect::<Vec<_>>();
    for expected in [
        "$ai_input_token_price",
        "$ai_output_token_price",
        "$ai_cache_read_token_price",
        "$ai_cache_write_token_price",
        "$ai_reasoning_tokens",
    ] {
        assert!(attribute_keys.contains(&expected), "OTLP attribute {expected} missing from {attribute_keys:?}");
    }

    assert!(metric_names.contains(&"gen_ai.client.operation.duration"));
    assert!(metric_names.contains(&"gen_ai.client.token.usage"));

    assert_eq!(string_attribute(turn, "gen_ai.agent.name"), Some("PatchWaveFix"));
    assert_eq!(chat.parent_span_id, turn.span_id);
    assert_eq!(string_attribute(chat, GEN_AI_SYSTEM_INSTRUCTIONS), Some(SYSTEM_INSTRUCTIONS_JSON));
    assert_eq!(string_attribute(chat, AETHER_SYSTEM_INSTRUCTIONS_SHA256), Some(SYSTEM_PROMPT_SHA256));
    assert_eq!(exports.trace_headers, vec!["collector-test"]);
    assert_eq!(exports.metric_headers, vec!["collector-test"]);
}

fn collector_config(collector: &FakeOtlpCollector) -> TelemetryConfig {
    TelemetryConfig {
        endpoint: Some(collector.endpoint()),
        traces_endpoint: None,
        metrics_endpoint: None,
        headers: HashMap::new(),
        service_name: "aether-test".to_string(),
        service_version: "test".to_string(),
        sample_ratio: 1.0,
        content: ContentCaptureSettings::default(),
        trace_context: None,
        traces_enabled: true,
        metrics_enabled: true,
    }
}

fn capturing_collector_config(collector: &FakeOtlpCollector) -> TelemetryConfig {
    TelemetryConfig { content: all_content(), ..collector_config(collector) }
}

fn test_headers(value: &str) -> HashMap<String, String> {
    HashMap::from([("x-telemetry-test".to_string(), value.to_string())])
}

fn string_attribute<'a>(span: &'a Span, key: &str) -> Option<&'a str> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;

    span.attributes.iter().find(|attribute| attribute.key == key).and_then(|attribute| {
        match &attribute.value.as_ref()?.value {
            Some(Value::StringValue(value)) => Some(value.as_str()),
            _ => None,
        }
    })
}

fn trace_spans(exports: &Exports) -> Vec<&Span> {
    exports
        .traces
        .iter()
        .flat_map(|request| &request.resource_spans)
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .collect()
}

fn assert_propagated_hierarchy(spans: &[&Span], expected_trace_id: &[u8], expected_parent_span_id: &[u8]) {
    assert_eq!(spans.len(), 4);
    assert!(spans.iter().all(|span| span.trace_id == expected_trace_id));
    let roots = spans.iter().filter(|span| span.name == "invoke_agent").copied().collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().all(|span| span.parent_span_id == expected_parent_span_id));
    assert!(roots.iter().all(|span| span.trace_state == "vendor=value"));
    for child in spans.iter().filter(|span| span.name != "invoke_agent") {
        assert!(roots.iter().any(|root| child.parent_span_id == root.span_id));
    }
    let span_ids = spans.iter().map(|span| span.span_id.clone()).collect::<std::collections::HashSet<_>>();
    assert_eq!(span_ids.len(), spans.len());
    assert!(span_ids.iter().all(|span_id| !span_id.is_empty() && span_id.iter().any(|byte| *byte != 0)));
}

/// Feeds one complete turn through a fresh observer, whose spans the runtime
/// exports once the observer is dropped.
fn observe_a_turn(
    runtime: &TelemetryRuntime,
    parent: Option<&TraceContext>,
    agent_name: Option<&str>,
    system_prompt: Option<&str>,
) {
    let mut observer = runtime.observer_factory().agent(agent_name, parent);
    if let Some(prompt) = system_prompt {
        observer.on_system_prompt(prompt);
    }
    for event in events() {
        observer.on_event(&event);
    }
}

fn events() -> Vec<AgentEvent> {
    vec![
        AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            model: ModelIdentity {
                provider: Some("anthropic".to_string()),
                model_id: Some("test-model".to_string()),
                pricing: Some(ModelPricing {
                    input_per_million: 3.0,
                    output_per_million: 15.0,
                    cache_read_per_million: Some(0.3),
                    cache_write_per_million: Some(3.75),
                }),
            },
            display_name: "test-model".to_string(),
            attempt: 0,
            max_attempts: 1,
        }),
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed {
                stop_reason: None,
                usage: Some(TokenUsage {
                    cache_read_tokens: Some(4.into()),
                    cache_creation_tokens: Some(2.into()),
                    reasoning_tokens: Some(3.into()),
                    ..TokenUsage::new(10, 5)
                }),
            },
        }),
        AgentEvent::turn_ended(TurnOutcome::Completed),
    ]
}

struct FakeOtlpCollector {
    address: std::net::SocketAddr,
    exports: Arc<Mutex<Exports>>,
    server: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Exports {
    traces: Vec<ExportTraceServiceRequest>,
    metrics: Vec<ExportMetricsServiceRequest>,
    trace_headers: Vec<String>,
    metric_headers: Vec<String>,
}

impl FakeOtlpCollector {
    async fn start() -> Self {
        let exports = Arc::new(Mutex::new(Exports::default()));
        let app = Router::new()
            .route("/v1/traces", post(export_traces))
            .route("/i/v0/ai/otel", post(export_traces))
            .route("/custom/metrics", post(export_metrics))
            .route("/v1/metrics", post(export_metrics))
            .with_state(exports.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fake collector");
        let address = listener.local_addr().expect("collector address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fake collector");
        });

        Self { address, exports, server }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    fn signal_specific_traces_endpoint(&self) -> String {
        format!("http://{}/i/v0/ai/otel", self.address)
    }

    fn signal_specific_metrics_endpoint(&self) -> String {
        format!("http://{}/custom/metrics", self.address)
    }

    fn exports(&self) -> Exports {
        let mut exports = self.exports.lock().expect("collector state");
        std::mem::take(&mut *exports)
    }
}

impl Drop for FakeOtlpCollector {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn export_traces(State(exports): State<Arc<Mutex<Exports>>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let mut exports = exports.lock().expect("collector state");
    exports.traces.push(ExportTraceServiceRequest::decode(body).expect("valid OTLP trace payload"));
    exports.trace_headers.push(header_value(&headers));
    StatusCode::OK
}

async fn export_metrics(State(exports): State<Arc<Mutex<Exports>>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let mut exports = exports.lock().expect("collector state");
    exports.metrics.push(ExportMetricsServiceRequest::decode(body).expect("valid OTLP metric payload"));
    exports.metric_headers.push(header_value(&headers));
    StatusCode::OK
}

fn header_value(headers: &HeaderMap) -> String {
    headers.get("x-telemetry-test").expect("configured header").to_str().expect("valid header").to_string()
}
