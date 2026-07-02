use aether_project::{OtlpProtocol as SettingsOtlpProtocol, TelemetrySettings};
use aether_telemetry::{OtlpProtocol, TelemetryInitError, TelemetryRuntime};
use std::sync::Arc;

pub(crate) fn build_telemetry_runtime(
    settings: &TelemetrySettings,
) -> Result<Option<Arc<TelemetryRuntime>>, TelemetryInitError> {
    if !settings.effective_enabled() {
        return Ok(None);
    }

    let endpoint = settings.otlp.endpoint.as_deref().unwrap_or_default();
    TelemetryRuntime::builder(endpoint)
        .service_name(settings.service_name.as_deref().unwrap_or("aether"))
        .sample_ratio(settings.sample_ratio)
        .capture_content(settings.capture_content)
        .protocol(map_protocol(settings.otlp.protocol))
        .headers(settings.otlp.headers.clone())
        .traces_enabled(settings.traces.enabled)
        .metrics_enabled(settings.metrics.enabled)
        .build()
        .map(|runtime| Some(Arc::new(runtime)))
}

fn map_protocol(protocol: SettingsOtlpProtocol) -> OtlpProtocol {
    match protocol {
        SettingsOtlpProtocol::Grpc => OtlpProtocol::Grpc,
        SettingsOtlpProtocol::HttpProtobuf => OtlpProtocol::HttpProtobuf,
    }
}
