use std::error::Error;
use std::process::{Command, Output};

const INVALID_TRACEPARENT: &str = "00-00000000000000000000000000000000-0123456789abcdef-01";
const VALID_TRACEPARENT: &str = "00-00112233445566778899aabbccddeeff-0123456789abcdef-01";
const TELEMETRY_SETTINGS: &str = r#""telemetry":{"otlp":{"endpoint":"http://127.0.0.1:1"}}"#;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn acp_rejects_an_invalid_trace_context_when_telemetry_is_enabled() -> TestResult {
    let options = format!(
        r#"{{"settings":{{"credentialsStore":{{"type":"memory"}},"agents":[],{TELEMETRY_SETTINGS}}},"traceContext":{{"traceparent":"{INVALID_TRACEPARENT}"}}}}"#
    );
    let output = aether(&["acp", "--options-json", &options]).output()?;
    assert_rejected(&output, "traceparent");
    Ok(())
}

#[test]
fn headless_rejects_an_invalid_trace_context_when_telemetry_is_enabled() -> TestResult {
    let options = format!(
        r#"{{"prompt":"hello","settings":{{"agents":[],{TELEMETRY_SETTINGS}}},"traceContext":{{"traceparent":"{INVALID_TRACEPARENT}"}}}}"#
    );
    let output = aether(&["headless", "--options-json", &options]).output()?;
    assert_rejected(&output, "traceparent");
    Ok(())
}

#[test]
fn headless_rejects_an_invalid_tracestate_when_telemetry_is_enabled() -> TestResult {
    let options = format!(
        r#"{{"prompt":"hello","settings":{{"agents":[],{TELEMETRY_SETTINGS}}},"traceContext":{{"traceparent":"{VALID_TRACEPARENT}","tracestate":"not a valid tracestate"}}}}"#
    );
    let output = aether(&["headless", "--options-json", &options]).output()?;
    assert_rejected(&output, "tracestate");
    Ok(())
}

fn aether(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command.args(args);
    command
}

fn assert_rejected(output: &Output, header: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "invalid trace context succeeded: {stderr}");
    assert!(stderr.contains(&format!("telemetry trace context has an invalid {header} header")), "stderr: {stderr}");
}
