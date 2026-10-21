use crate::TestResult;
use crate::acp_process::{AcpProcess, AcpProcessBuilder, initialize_params};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Error as IoError, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Stdio;

const SETTINGS: &str = r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#;

#[test]
fn socket_backed_stdio_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let (mut child_stdout_reader, child_stdout) = UnixStream::pair()?;
    let (mut child_stdin_writer, child_stdin) = UnixStream::pair()?;
    let mut child = acp_builder(log_dir.path())
        .into_command()
        .stdin(Stdio::from(OwnedFd::from(child_stdin)))
        .stdout(Stdio::from(OwnedFd::from(child_stdout)))
        .spawn()?;

    child_stdin_writer.write_all(
        format!("{}\n", json!({ "jsonrpc": "2.0", "method": "initialize", "params": initialize_params()?, "id": 1 }))
            .as_bytes(),
    )?;
    child_stdin_writer.flush()?;

    let mut response = String::new();
    BufReader::new(&mut child_stdout_reader).read_line(&mut response)?;
    assert_initialize_response(&serde_json::from_str(&response)?);
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

#[test]
fn pipe_backed_stdio_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let mut agent = acp_builder(log_dir.path()).spawn()?;

    assert_initialize_response(&json!({ "id": 1, "result": agent.initialize()? }));
    Ok(())
}

#[test]
fn options_json_with_trace_context_serves_acp() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let options = format!(
        r#"{{"logDir":{},"settings":{{"credentialsStore":{{"type":"memory"}},"agents":[]}},"traceContext":{{"traceparent":"00-00112233445566778899aabbccddeeff-0123456789abcdef-01","tracestate":"vendor=value"}}}}"#,
        serde_json::to_string(log_dir.path())?
    );
    let mut agent = AcpProcess::builder().arg("--options-json").arg(options).spawn()?;

    assert_initialize_response(&json!({ "id": 1, "result": agent.initialize()? }));
    Ok(())
}

#[test]
fn pipe_backed_stdio_rejects_removed_v1_methods() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let mut agent = acp_builder(log_dir.path()).spawn()?;
    agent.initialize()?;

    agent.send(&json!([
        {"jsonrpc": "2.0", "method": "session/load", "params": {}, "id": 2},
        {"jsonrpc": "2.0", "method": "authenticate", "params": {}, "id": 3}
    ]))?;

    let response = agent.read_message()?;
    let responses = response.as_array().ok_or_else(|| IoError::other("expected a grouped response array"))?;
    assert_eq!(responses.len(), 2, "expected one response per batch entry: {response}");
    for (index, id) in [2u64, 3].into_iter().enumerate() {
        assert_eq!(responses[index]["id"], json!(id), "response order should match the batch: {response}");
        let message = responses[index]["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("Method not found"), "expected Method not found: {response}");
    }
    Ok(())
}

#[test]
fn an_agent_with_an_open_session_exits_once_its_client_hangs_up() -> TestResult {
    let home = tempfile::tempdir()?;
    let mut agent = acp_builder(home.path()).env("HOME", home.path()).env("AETHER_HOME", home.path()).spawn()?;
    agent.initialize()?;
    agent.request("session/new", &json!({ "cwd": home.path(), "mcpServers": [] }))?;

    // Waits on no clock: an agent that outlives its client makes the test hang.
    let status = agent.hang_up()?;

    assert!(status.success(), "{status}");
    Ok(())
}

fn acp_builder(log_dir: &Path) -> AcpProcessBuilder {
    AcpProcess::builder().log_dir(log_dir).settings_json(SETTINGS)
}

fn assert_initialize_response(response: &Value) {
    assert_eq!(response["id"], json!(1), "response should echo the request id: {response}");
    assert_eq!(response["result"]["protocolVersion"], 2);
    assert_eq!(response["result"]["info"]["name"], "Aether");
    assert!(response["result"]["capabilities"]["session"].is_object());
    assert!(response["result"]["capabilities"].get("loadSession").is_none());
}
