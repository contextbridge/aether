use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::InitializeRequest;
use std::error::Error;
use std::io::{BufRead, BufReader, Error as IoError, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

#[test]
fn socket_backed_stdio_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let (mut child_stdout_reader, child_stdout) = UnixStream::pair()?;
    let (mut child_stdin_writer, child_stdin) = UnixStream::pair()?;
    let mut child = acp_command(log_dir.path())
        .stdin(Stdio::from(OwnedFd::from(child_stdin)))
        .stdout(Stdio::from(OwnedFd::from(child_stdout)))
        .spawn()?;

    child_stdin_writer.write_all(initialize_line()?.as_bytes())?;
    child_stdin_writer.flush()?;

    let mut response = String::new();
    BufReader::new(&mut child_stdout_reader).read_line(&mut response)?;
    assert_initialize_response(&response)?;
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

#[test]
fn pipe_backed_stdio_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    assert_serves_initialize(acp_command(log_dir.path()))
}

#[test]
fn options_json_with_trace_context_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let options = format!(
        r#"{{"logDir":{},"settings":{{"credentialsStore":{{"type":"memory"}},"agents":[]}},"traceContext":{{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"vendor=value"}}}}"#,
        serde_json::to_string(log_dir.path())?
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command.arg("acp").arg("--options-json").arg(options).stderr(Stdio::null());
    assert_serves_initialize(command)
}

#[test]
fn pipe_backed_stdio_rejects_unknown_non_underscore_methods() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let mut child = acp_command(log_dir.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| IoError::other("child stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| IoError::other("child stdout"))?;
    let mut reader = BufReader::new(stdout);

    stdin.write_all(initialize_line()?.as_bytes())?;
    stdin.flush()?;

    let mut response = String::new();
    reader.read_line(&mut response)?;
    assert_initialize_response(&response)?;

    stdin.write_all(non_underscore_batch_line()?.as_bytes())?;
    stdin.flush()?;

    response.clear();
    reader.read_line(&mut response)?;
    let responses: serde_json::Value = serde_json::from_str(&response)?;
    let responses = responses.as_array().ok_or_else(|| IoError::other("expected a grouped response array"))?;
    assert_eq!(responses.len(), 2, "expected one response per batch entry: {response}");
    for (index, id) in [2u64, 3].into_iter().enumerate() {
        assert_eq!(responses[index]["id"], serde_json::json!(id), "response order should match the batch: {response}");
        let message = responses[index]["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("Method not found"), "expected Method not found: {response}");
    }

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn assert_serves_initialize(mut command: Command) -> TestResult {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| IoError::other("child stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| IoError::other("child stdout"))?;

    stdin.write_all(initialize_line()?.as_bytes())?;
    stdin.flush()?;

    let mut response = String::new();
    BufReader::new(stdout).read_line(&mut response)?;
    assert_initialize_response(&response)?;

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn acp_command(log_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("acp")
        .arg("--log-dir")
        .arg(log_dir)
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .stderr(Stdio::null());
    command
}

fn initialize_line() -> TestResult<String> {
    let params = serde_json::to_value(InitializeRequest::new(ProtocolVersion::V1))?;
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": params,
        "id": 1
    });
    Ok(format!("{}\n", serde_json::to_string(&line)?))
}

fn non_underscore_batch_line() -> TestResult<String> {
    let batch = serde_json::json!([
        {"jsonrpc": "2.0", "method": "unknown/one", "params": {}, "id": 2},
        {"jsonrpc": "2.0", "method": "unknown/two", "params": {}, "id": 3}
    ]);
    Ok(format!("{}\n", serde_json::to_string(&batch)?))
}

fn assert_initialize_response(line: &str) -> TestResult {
    let response: serde_json::Value = serde_json::from_str(line)?;
    assert_eq!(response["id"], serde_json::json!(1), "response should echo the request id: {response}");
    assert!(response.get("result").is_some(), "initialize should return a result: {response}");
    Ok(())
}
