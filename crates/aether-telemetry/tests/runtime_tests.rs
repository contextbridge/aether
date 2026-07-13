use aether_telemetry::{TelemetryConfig, TelemetryInitError, TelemetryRuntime};
use std::collections::HashMap;

#[test]
fn disabled_signals_do_not_require_an_endpoint_or_exporters() {
    let runtime = TelemetryRuntime::new(&TelemetryConfig {
        endpoint: None,
        traces_endpoint: None,
        metrics_endpoint: None,
        traces_enabled: false,
        metrics_enabled: false,
        ..test_config()
    })
    .expect("disabled signals do not need exporters");

    runtime.shutdown().expect("no-exporter providers shut down cleanly");
}

#[test]
fn enabled_signals_require_an_endpoint() {
    for endpoint in [None, Some(String::new())] {
        let result = TelemetryRuntime::new(&TelemetryConfig { endpoint, ..test_config() });

        assert!(matches!(result, Err(TelemetryInitError::MissingOtlpEndpoint)));
    }
}

#[test]
fn enabled_signals_validate_their_endpoint() {
    for (traces_enabled, metrics_enabled) in [(true, false), (false, true)] {
        let result = TelemetryRuntime::new(&TelemetryConfig {
            endpoint: Some("not a valid endpoint".to_string()),
            traces_endpoint: None,
            metrics_endpoint: None,
            traces_enabled,
            metrics_enabled,
            ..test_config()
        });

        assert!(matches!(
            (traces_enabled, result),
            (true, Err(TelemetryInitError::TraceExporter(_))) | (false, Err(TelemetryInitError::MetricExporter(_)))
        ));
    }
}

#[test]
fn sample_ratio_must_be_between_zero_and_one() {
    for ratio in [-0.1, 1.1, f64::NAN] {
        let result = TelemetryRuntime::new(&TelemetryConfig { sample_ratio: ratio, ..test_config() });

        assert!(matches!(result, Err(TelemetryInitError::InvalidSampleRatio(_))));
    }
}

#[test]
fn headers_are_validated_even_when_signals_are_disabled() {
    let config = |headers: HashMap<String, String>| TelemetryConfig {
        headers,
        traces_enabled: false,
        metrics_enabled: false,
        ..test_config()
    };

    let invalid_name = TelemetryRuntime::new(&config(HashMap::from([("bad header".to_string(), "value".to_string())])));
    assert!(matches!(invalid_name, Err(TelemetryInitError::InvalidHeaderName(name)) if name == "bad header"));

    let invalid_value =
        TelemetryRuntime::new(&config(HashMap::from([("authorization".to_string(), "bad\nvalue".to_string())])));
    assert!(matches!(invalid_value, Err(TelemetryInitError::InvalidHeaderValue(name)) if name == "authorization"));
}

fn test_config() -> TelemetryConfig {
    TelemetryConfig {
        endpoint: Some("http://localhost:4318".to_string()),
        traces_endpoint: None,
        metrics_endpoint: None,
        headers: HashMap::new(),
        service_name: "aether".to_string(),
        service_version: "test".to_string(),
        sample_ratio: 1.0,
        capture_content: false,
        traces_enabled: true,
        metrics_enabled: true,
    }
}
