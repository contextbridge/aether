use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aether_core::events::{AgentEvent, LlmCallOutcome, LlmCallPurpose, TurnEvent, TurnOutcome};
use aether_telemetry::{TelemetryConfig, TelemetryRuntime};
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use llm::TokenUsage;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;

#[tokio::test(flavor = "multi_thread")]
async fn runtime_exports_metrics_to_a_signal_specific_endpoint() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        endpoint: None,
        traces_endpoint: None,
        metrics_endpoint: Some(collector.signal_specific_metrics_endpoint()),
        headers: HashMap::from([("x-telemetry-test".to_string(), "signal-specific".to_string())]),
        service_name: "aether-test".to_string(),
        service_version: "test".to_string(),
        sample_ratio: 1.0,
        capture_content: false,
        traces_enabled: false,
        metrics_enabled: true,
    })
    .expect("runtime initializes against a signal-specific endpoint");
    {
        let factory = runtime.observer_factory();
        let mut observer = factory();
        for event in events() {
            observer.on_event(&event);
        }
    };

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
        metrics_endpoint: None,
        headers: HashMap::from([("x-telemetry-test".to_string(), "signal-specific".to_string())]),
        service_name: "aether-test".to_string(),
        service_version: "test".to_string(),
        sample_ratio: 1.0,
        capture_content: false,
        traces_enabled: true,
        metrics_enabled: false,
    })
    .expect("runtime initializes against a signal-specific endpoint");
    {
        let factory = runtime.observer_factory();
        let mut observer = factory();
        for event in events() {
            observer.on_event(&event);
        }
    };

    runtime.shutdown().expect("runtime flushes traces");
    let exports = collector.exports();

    assert_eq!(exports.traces.len(), 1);
    assert!(exports.metrics.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_exports_genai_spans_and_metrics_to_an_otlp_collector() {
    let collector = FakeOtlpCollector::start().await;
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        endpoint: Some(collector.endpoint()),
        traces_endpoint: None,
        metrics_endpoint: None,
        headers: HashMap::from([("x-telemetry-test".to_string(), "collector-test".to_string())]),
        service_name: "aether-test".to_string(),
        service_version: "test".to_string(),
        sample_ratio: 1.0,
        capture_content: false,
        traces_enabled: true,
        metrics_enabled: true,
    })
    .expect("runtime initializes against collector");
    {
        let factory = runtime.observer_factory();
        let mut observer = factory();
        for event in events() {
            observer.on_event(&event);
        }
    };

    runtime.shutdown().expect("runtime flushes both signals");
    let exports = collector.exports();
    let span_names = exports
        .traces
        .iter()
        .flat_map(|request| &request.resource_spans)
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .map(|span| span.name.as_str())
        .collect::<Vec<_>>();

    let metric_names = exports
        .metrics
        .iter()
        .flat_map(|request| &request.resource_metrics)
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .map(|metric| metric.name.as_str())
        .collect::<Vec<_>>();

    assert!(span_names.contains(&"invoke_agent"));
    assert!(span_names.contains(&"chat test-model"));

    assert!(metric_names.contains(&"gen_ai.client.operation.duration"));
    assert!(metric_names.contains(&"gen_ai.client.token.usage"));

    assert_eq!(exports.trace_headers, vec!["collector-test"]);
    assert_eq!(exports.metric_headers, vec!["collector-test"]);
}

fn events() -> Vec<AgentEvent> {
    vec![
        AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            provider: Some("anthropic".to_string()),
            model: Some("test-model".to_string()),
            display_name: "test-model".to_string(),
            attempt: 0,
            max_attempts: 1,
        }),
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: Some(TokenUsage::new(10, 5)) },
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
