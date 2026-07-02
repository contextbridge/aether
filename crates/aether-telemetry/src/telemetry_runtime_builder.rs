use crate::error::TelemetryInitError;
use crate::gen_ai_metrics::GenAiMetrics;
use crate::genai_semconv;
use crate::telemetry_runtime::{MetricsWithProvider, TelemetryRuntime, TracerWithProvider};
use opentelemetry::KeyValue;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::tonic_types::metadata::MetadataMap;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::collections::HashMap;

pub struct TelemetryRuntimeBuilder {
    endpoint: String,
    protocol: OtlpProtocol,
    headers: HashMap<String, String>,
    service_name: String,
    service_version: String,
    sample_ratio: f64,
    capture_content: bool,
    traces_enabled: bool,
    metrics_enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    HttpProtobuf,
}

impl TelemetryRuntimeBuilder {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            protocol: OtlpProtocol::default(),
            headers: HashMap::new(),
            service_name: "aether".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            sample_ratio: 1.0,
            capture_content: false,
            traces_enabled: true,
            metrics_enabled: true,
        }
    }

    pub fn service_name(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self
    }

    pub fn service_version(mut self, service_version: impl Into<String>) -> Self {
        self.service_version = service_version.into();
        self
    }

    pub fn sample_ratio(mut self, sample_ratio: f64) -> Self {
        self.sample_ratio = sample_ratio;
        self
    }

    pub fn capture_content(mut self, capture_content: bool) -> Self {
        self.capture_content = capture_content;
        self
    }

    pub fn protocol(mut self, protocol: OtlpProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn headers(mut self, headers: impl IntoIterator<Item = (String, String)>) -> Self {
        self.headers = headers.into_iter().collect();
        self
    }

    pub fn traces_enabled(mut self, enabled: bool) -> Self {
        self.traces_enabled = enabled;
        self
    }

    pub fn metrics_enabled(mut self, enabled: bool) -> Self {
        self.metrics_enabled = enabled;
        self
    }

    pub fn build(self) -> Result<TelemetryRuntime, TelemetryInitError> {
        self.validate()?;
        let metadata = parse_headers(&self.headers)?;
        let resource = Resource::builder()
            .with_service_name(self.service_name.clone())
            .with_attribute(KeyValue::new("service.version", self.service_version.clone()))
            .with_attribute(KeyValue::new(
                "telemetry.semconv.gen_ai.schema_url",
                genai_semconv::GENAI_SEMCONV_SCHEMA_URL,
            ))
            .build();
        let traces = self.traces_enabled.then(|| self.build_traces(resource.clone(), metadata.clone())).transpose()?;
        let metrics = self.metrics_enabled.then(|| self.build_metrics(resource, metadata)).transpose()?;

        Ok(TelemetryRuntime { traces, metrics, capture_content: self.capture_content })
    }

    fn validate(&self) -> Result<(), TelemetryInitError> {
        if self.endpoint.is_empty() {
            return Err(TelemetryInitError::MissingOtlpEndpoint);
        }
        if !(0.0..=1.0).contains(&self.sample_ratio) {
            return Err(TelemetryInitError::InvalidSampleRatio(self.sample_ratio));
        }
        Ok(())
    }

    fn build_traces(
        &self,
        resource: Resource,
        metadata: MetadataMap,
    ) -> Result<TracerWithProvider, TelemetryInitError> {
        let exporter = match self.protocol {
            OtlpProtocol::Grpc => {
                SpanExporter::builder().with_tonic().with_endpoint(&self.endpoint).with_metadata(metadata).build()
            }

            OtlpProtocol::HttpProtobuf => SpanExporter::builder()
                .with_http()
                .with_endpoint(&self.endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_headers(self.headers.clone())
                .build(),
        }
        .map_err(|error| TelemetryInitError::TraceExporter(error.to_string()))?;

        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_sampler(Sampler::TraceIdRatioBased(self.sample_ratio))
            .with_batch_exporter(exporter)
            .build();

        let tracer = provider.tracer("aether.genai");
        Ok(TracerWithProvider { provider, tracer })
    }

    fn build_metrics(
        &self,
        resource: Resource,
        metadata: MetadataMap,
    ) -> Result<MetricsWithProvider, TelemetryInitError> {
        let exporter = match self.protocol {
            OtlpProtocol::Grpc => opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_endpoint(&self.endpoint)
                .with_metadata(metadata)
                .build(),
            OtlpProtocol::HttpProtobuf => opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_endpoint(&self.endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_headers(self.headers.clone())
                .build(),
        }
        .map_err(|error| TelemetryInitError::MetricExporter(error.to_string()))?;

        let provider = SdkMeterProvider::builder().with_resource(resource).with_periodic_exporter(exporter).build();
        let instruments = GenAiMetrics::new(&provider.meter("aether.genai"));
        Ok(MetricsWithProvider { provider, instruments })
    }
}

/// Parses the configured headers with the same `http` types the exporters use,
/// so anything accepted here is transportable by construction.
fn parse_headers(headers: &HashMap<String, String>) -> Result<MetadataMap, TelemetryInitError> {
    let mut parsed = http::HeaderMap::new();
    for (key, value) in headers {
        let name = http::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| TelemetryInitError::InvalidHeaderName(key.clone()))?;
        let value =
            http::HeaderValue::from_str(value).map_err(|_| TelemetryInitError::InvalidHeaderValue(key.clone()))?;
        parsed.insert(name, value);
    }
    Ok(MetadataMap::from_headers(parsed))
}
