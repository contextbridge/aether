use aether_telemetry::{AgentTraceContext, TelemetryConfig, TelemetryInitError, TelemetryRuntime};
use std::collections::HashMap;

#[test]
fn round_trips_agent_trace_context_carriers() {
    let context: AgentTraceContext = serde_json::from_str(
        r#"{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"vendor=value"}"#,
    )
    .expect("valid trace context carrier");

    assert_eq!(
        serde_json::to_string(&context).expect("serializes trace context"),
        r#"{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"vendor=value"}"#
    );
}

#[test]
fn round_trips_trace_id_only_contexts() {
    let context: AgentTraceContext =
        serde_json::from_str(r#"{"traceId":"00112233445566778899aabbccddeeff"}"#).expect("valid trace ID context");

    assert_eq!(
        serde_json::to_string(&context).expect("serializes trace ID context"),
        r#"{"traceId":"00112233445566778899aabbccddeeff"}"#
    );
}

#[test]
fn rejects_invalid_agent_trace_contexts_when_initializing_runtime() {
    for (value, header) in [
        (r#"{"traceparent":"INVALID"}"#, "traceparent"),
        (r#"{"traceparent":"00-00000000000000000000000000000000-0123456789abcdef-01"}"#, "traceparent"),
        (r#"{"traceparent":"00-00112233445566778899aabbccddeeff-0000000000000000-01"}"#, "traceparent"),
        (r#"{"traceparent":"00-00112233445566778899AABBCCDDEEFF-0123456789abcdef-01"}"#, "traceparent"),
        (r#"{"traceId":"00000000000000000000000000000000"}"#, "traceId"),
        (r#"{"traceId":"00112233445566778899aabbccddee"}"#, "traceId"),
        (r#"{"traceId":"00112233445566778899AABBCCDDEEFF"}"#, "traceId"),
        (r#"{"traceId":"00112233445566778899aabbccddeezz"}"#, "traceId"),
        (
            r#"{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"invalid"}"#,
            "tracestate",
        ),
    ] {
        let trace_context = serde_json::from_str::<AgentTraceContext>(value).expect("valid JSON carrier");
        let result = TelemetryRuntime::new(&TelemetryConfig {
            trace_context: Some(trace_context),
            traces_enabled: false,
            metrics_enabled: false,
            ..test_config()
        });

        assert!(
            matches!(result, Err(TelemetryInitError::InvalidTraceContext(name)) if name == header),
            "expected an invalid {header} for {value}"
        );
    }
}

#[test]
fn rejects_mixed_trace_context_forms() {
    let result = serde_json::from_str::<AgentTraceContext>(
        r#"{"traceId":"00112233445566778899aabbccddeeff","traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01"}"#,
    );

    assert!(result.is_err());
}

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
        trace_context: None,
        traces_enabled: true,
        metrics_enabled: true,
    }
}
