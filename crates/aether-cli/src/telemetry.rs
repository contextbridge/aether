use aether_project::TelemetrySettings;
use aether_telemetry::{TelemetryConfig, TelemetryInitError, TelemetryRuntime};
use std::sync::Arc;

pub(crate) fn build_telemetry_runtime(
    settings: Option<&TelemetrySettings>,
) -> Result<Option<Arc<TelemetryRuntime>>, TelemetryInitError> {
    let Some(settings) = settings else {
        return Ok(None);
    };

    if !settings.effective_enabled() {
        return Ok(None);
    }

    TelemetryRuntime::new(&TelemetryConfig {
        endpoint: settings.otlp.endpoint.clone(),
        headers: settings.otlp.headers.clone().into_iter().collect(),
        service_name: settings.service_name().to_string(),
        service_version: env!("CARGO_PKG_VERSION").to_string(),
        sample_ratio: settings.sample_ratio(),
        capture_content: settings.capture_content(),
        traces_enabled: settings.traces_enabled(),
        metrics_enabled: settings.metrics_enabled(),
    })
    .map(|runtime| Some(Arc::new(runtime)))
}
