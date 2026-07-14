use utils::variables::VarError;

use opentelemetry_otlp::ExporterBuildError;
use opentelemetry_sdk::error::OTelSdkError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TelemetryInitError {
    #[error("failed to resolve telemetry configuration: {0}")]
    Variable(#[from] VarError),
    #[error("telemetry OTLP endpoint is required")]
    MissingOtlpEndpoint,
    #[error("telemetry sample ratio must be between 0.0 and 1.0, got {0}")]
    InvalidSampleRatio(f64),
    #[error("telemetry OTLP header name is invalid: {0}")]
    InvalidHeaderName(String),
    #[error("telemetry OTLP header value is invalid for header {0}")]
    InvalidHeaderValue(String),
    #[error("failed to initialize telemetry HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),
    #[error("failed to initialize trace exporter: {0}")]
    TraceExporter(#[source] ExporterBuildError),
    #[error("failed to initialize metric exporter: {0}")]
    MetricExporter(#[source] ExporterBuildError),
}

#[derive(Debug, Error)]
pub enum TelemetryShutdownError {
    #[error("failed to shutdown trace provider: {0}")]
    Trace(#[source] OTelSdkError),
    #[error("failed to shutdown metric provider: {0}")]
    Metric(#[source] OTelSdkError),
}
