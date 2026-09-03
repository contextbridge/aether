use aether_project::TelemetrySettings;
use aether_telemetry::{
    AgentTraceContext, ContentCaptureSettings, TelemetryConfig, TelemetryInitError, TelemetryRuntime,
};
use std::sync::Arc;
use utils::variables::Vars;

pub(crate) fn build_telemetry_runtime(
    settings: Option<&TelemetrySettings>,
    trace_context: Option<AgentTraceContext>,
) -> Result<Option<Arc<TelemetryRuntime>>, TelemetryInitError> {
    let Some(settings) = settings else {
        return Ok(None);
    };

    if !settings.effective_enabled() {
        return Ok(None);
    }

    TelemetryRuntime::new(&TelemetryConfig {
        endpoint: settings.otlp.endpoint.clone(),
        traces_endpoint: settings.otlp.traces_endpoint.clone(),
        metrics_endpoint: settings.otlp.metrics_endpoint.clone(),
        headers: settings.otlp.resolved_headers(&Vars::new())?.into_iter().collect(),
        service_name: settings.service_name().to_string(),
        service_version: env!("CARGO_PKG_VERSION").to_string(),
        sample_ratio: settings.sample_ratio(),
        content: ContentCaptureSettings {
            system_instructions: settings.content.system_instructions(),
            input_messages: settings.content.input_messages(),
            output_messages: settings.content.output_messages(),
            tool_definitions: settings.content.tool_definitions(),
            tool_calls: settings.content.tool_calls(),
        },
        trace_context,
        traces_enabled: settings.traces_enabled(),
        metrics_enabled: settings.metrics_enabled(),
    })
    .map(|runtime| Some(Arc::new(runtime)))
}
