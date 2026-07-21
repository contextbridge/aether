use crate::AgentTraceContext;
use crate::error::{TelemetryInitError, TelemetryShutdownError};
use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_constants;
use crate::otel_observer::OtelInstrumentation;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::{TraceContextExt, TraceId, TraceState, TracerProvider as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator, Sampler, SdkTracerProvider};
use std::collections::HashMap;
use std::str::FromStr;

pub struct TelemetryRuntime {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
    instrumentation: OtelInstrumentation,
}

/// Fully-resolved telemetry configuration. Every field is required: defaults
/// and settings-layer merging live in `TelemetrySettings`, so a value here is
/// always deliberate.
pub struct TelemetryConfig {
    /// Base URL for the OTLP/HTTP collector, required whenever a signal needs an exporter.
    pub endpoint: Option<String>,
    /// Exact OTLP/HTTP trace export URL. When set, this takes precedence over the
    /// trace URL derived from `endpoint`.
    pub traces_endpoint: Option<String>,
    /// Exact OTLP/HTTP metric export URL. When set, this takes precedence over the
    /// metric URL derived from `endpoint`.
    pub metrics_endpoint: Option<String>,
    pub headers: HashMap<String, String>,
    pub service_name: String,
    pub service_version: String,
    pub sample_ratio: f64,
    pub capture_content: bool,
    pub trace_context: Option<AgentTraceContext>,
    pub traces_enabled: bool,
    pub metrics_enabled: bool,
}

impl TelemetryRuntime {
    pub fn new(config: &TelemetryConfig) -> Result<Self, TelemetryInitError> {
        if !(0.0..=1.0).contains(&config.sample_ratio) {
            return Err(TelemetryInitError::InvalidSampleRatio(config.sample_ratio));
        }

        let trace_root = resolve_trace_root(config.trace_context.as_ref())?;
        let http_client = build_http_client(&config.headers)?;
        let resource = Resource::builder()
            .with_service_name(config.service_name.clone())
            .with_attribute(KeyValue::new(genai_constants::SERVICE_VERSION, config.service_version.clone()))
            .build();
        let scope = genai_constants::genai_instrumentation_scope(config.service_version.clone());
        let tracer_provider =
            build_tracer_provider(config, resource.clone(), http_client.clone(), trace_root.trace_id)?;
        let meter_provider = build_meter_provider(config, resource, http_client)?;
        let instrumentation = OtelInstrumentation {
            tracer: tracer_provider.tracer_with_scope(scope.clone()),
            metrics: GenAiMetrics::new(&meter_provider.meter_with_scope(scope)),
            capture_content: config.capture_content,
            root_parent: trace_root.parent,
        };

        Ok(Self { tracer_provider, meter_provider, instrumentation })
    }

    pub fn observer_factory(&self) -> aether_core::events::ObserverFactory {
        let instrumentation = self.instrumentation.clone();
        std::sync::Arc::new(move || Box::new(crate::OtelObserver::new(instrumentation.clone())))
    }

    /// Flushes and shuts down, logging any failure. For callers whose only
    /// recovery is logging.
    pub fn shutdown_or_log(&self) {
        if let Err(error) = self.shutdown() {
            tracing::error!("Failed to shutdown telemetry: {error}");
        }
    }

    /// Flushes and shuts down every signal. Both providers are always attempted;
    /// providers for disabled signals have no exporters.
    pub fn shutdown(&self) -> Result<(), TelemetryShutdownError> {
        let traces = self.tracer_provider.shutdown().map_err(TelemetryShutdownError::Trace);
        let metrics = self.meter_provider.shutdown().map_err(TelemetryShutdownError::Metric);
        traces.and(metrics)
    }
}

impl TelemetryConfig {
    fn signal_endpoint(&self, signal: &str) -> Result<String, TelemetryInitError> {
        let signal_specific_endpoint = match signal {
            "traces" => self.traces_endpoint.as_deref(),
            "metrics" => self.metrics_endpoint.as_deref(),
            _ => None,
        };
        if let Some(endpoint) = signal_specific_endpoint.filter(|endpoint| !endpoint.is_empty()) {
            return Ok(endpoint.to_string());
        }

        Ok(format!("{}/v1/{signal}", self.required_endpoint()?.trim_end_matches('/')))
    }

    fn required_endpoint(&self) -> Result<&str, TelemetryInitError> {
        self.endpoint.as_deref().filter(|endpoint| !endpoint.is_empty()).ok_or(TelemetryInitError::MissingOtlpEndpoint)
    }
}

#[derive(Default)]
struct TraceRoot {
    parent: Option<Context>,
    trace_id: Option<TraceId>,
}

#[derive(Debug)]
struct TraceIdGenerator {
    trace_id: TraceId,
    random: RandomIdGenerator,
}

impl IdGenerator for TraceIdGenerator {
    fn new_trace_id(&self) -> TraceId {
        self.trace_id
    }

    fn new_span_id(&self) -> opentelemetry::trace::SpanId {
        self.random.new_span_id()
    }
}

fn resolve_trace_root(trace_context: Option<&AgentTraceContext>) -> Result<TraceRoot, TelemetryInitError> {
    let Some(trace_context) = trace_context else {
        return Ok(TraceRoot::default());
    };

    match trace_context {
        AgentTraceContext::Parent { tracestate, .. } => {
            if let Some(tracestate) = tracestate {
                TraceState::from_str(tracestate).map_err(|_| TelemetryInitError::InvalidTraceContext("tracestate"))?;
            }

            let context = TraceContextPropagator::new().extract(trace_context);
            if !context.span().span_context().is_valid() {
                return Err(TelemetryInitError::InvalidTraceContext("traceparent"));
            }

            Ok(TraceRoot { parent: Some(context), trace_id: None })
        }
        AgentTraceContext::Root { trace_id } => {
            let trace_id = parse_trace_id(trace_id)?;
            Ok(TraceRoot { parent: None, trace_id: Some(trace_id) })
        }
    }
}

fn parse_trace_id(value: &str) -> Result<TraceId, TelemetryInitError> {
    let valid_hex =
        value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid_hex {
        return Err(TelemetryInitError::InvalidTraceContext("traceId"));
    }

    let trace_id = TraceId::from_hex(value).map_err(|_| TelemetryInitError::InvalidTraceContext("traceId"))?;
    if trace_id == TraceId::INVALID {
        return Err(TelemetryInitError::InvalidTraceContext("traceId"));
    }

    Ok(trace_id)
}

fn build_tracer_provider(
    config: &TelemetryConfig,
    resource: Resource,
    http_client: reqwest::Client,
    trace_id: Option<TraceId>,
) -> Result<SdkTracerProvider, TelemetryInitError> {
    let builder = SdkTracerProvider::builder().with_resource(resource);
    let builder = match trace_id {
        Some(trace_id) => {
            builder.with_id_generator(TraceIdGenerator { trace_id, random: RandomIdGenerator::default() })
        }
        None => builder,
    };
    if !config.traces_enabled {
        return Ok(builder.with_sampler(Sampler::AlwaysOff).build());
    }

    let endpoint = config.signal_endpoint("traces")?;
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_http_client(http_client)
        .build()
        .map_err(TelemetryInitError::TraceExporter)?;

    Ok(builder
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.sample_ratio))))
        .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
        .build())
}

fn build_meter_provider(
    config: &TelemetryConfig,
    resource: Resource,
    http_client: reqwest::Client,
) -> Result<SdkMeterProvider, TelemetryInitError> {
    let builder = SdkMeterProvider::builder().with_resource(resource);
    if !config.metrics_enabled {
        return Ok(builder.build());
    }

    let endpoint = config.signal_endpoint("metrics")?;
    let exporter = MetricExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_http_client(http_client)
        .build()
        .map_err(TelemetryInitError::MetricExporter)?;

    Ok(builder.with_reader(PeriodicReader::builder(exporter, Tokio).build()).build())
}

fn build_http_client(headers: &HashMap<String, String>) -> Result<reqwest::Client, TelemetryInitError> {
    let mut parsed = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| TelemetryInitError::InvalidHeaderName(key.clone()))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| TelemetryInitError::InvalidHeaderValue(key.clone()))?;
        parsed.insert(name, value);
    }
    reqwest::Client::builder().default_headers(parsed).build().map_err(TelemetryInitError::HttpClient)
}
