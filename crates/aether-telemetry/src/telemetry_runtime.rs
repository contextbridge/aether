use crate::error::TelemetryShutdownError;
use crate::gen_ai_metrics::GenAiMetrics;
use crate::observer::OtelInstrumentation;
use crate::telemetry_runtime_builder::TelemetryRuntimeBuilder;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};

pub struct TelemetryRuntime {
    pub(crate) traces: Option<TracerWithProvider>,
    pub(crate) metrics: Option<MetricsWithProvider>,
    pub(crate) capture_content: bool,
}

impl TelemetryRuntime {
    pub fn builder(endpoint: impl Into<String>) -> TelemetryRuntimeBuilder {
        TelemetryRuntimeBuilder::new(endpoint)
    }

    pub fn instrumentation(&self) -> OtelInstrumentation {
        OtelInstrumentation {
            tracer: self.traces.as_ref().map(|traces| traces.tracer.clone()),
            metrics: self.metrics.as_ref().map(|metrics| metrics.instruments.clone()),
            capture_content: self.capture_content,
        }
    }

    /// Flushes and shuts down, logging any failure. For callers whose only
    /// recovery is logging.
    pub fn shutdown_or_log(&self) {
        if let Err(error) = self.shutdown() {
            tracing::error!("Failed to shutdown telemetry: {error}");
        }
    }

    /// Flushes and shuts down every enabled signal. Both signals are always
    /// attempted; the first error is returned.
    pub fn shutdown(&self) -> Result<(), TelemetryShutdownError> {
        let traces = self.traces.as_ref().map_or(Ok(()), |traces| {
            traces.provider.shutdown().map_err(|error| TelemetryShutdownError::Trace(error.to_string()))
        });
        let metrics = self.metrics.as_ref().map_or(Ok(()), |metrics| {
            metrics.provider.shutdown().map_err(|error| TelemetryShutdownError::Metric(error.to_string()))
        });
        traces.and(metrics)
    }
}

pub(crate) struct TracerWithProvider {
    pub(crate) provider: SdkTracerProvider,
    pub(crate) tracer: SdkTracer,
}

pub(crate) struct MetricsWithProvider {
    pub(crate) provider: SdkMeterProvider,
    pub(crate) instruments: GenAiMetrics,
}
