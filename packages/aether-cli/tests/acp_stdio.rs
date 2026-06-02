use agent_client_protocol::JsonRpcMessage;
use agent_client_protocol::jsonrpcmsg::{Id, Params, Request};
use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion};
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
    let mut child = acp_command(log_dir.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;

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
    command.arg("acp").arg("--log-dir").arg(log_dir).arg("--credential-store").arg("memory").stderr(Stdio::null());
    command
}

fn initialize_line() -> TestResult<String> {
    let message = InitializeRequest::new(ProtocolVersion::LATEST).to_untyped_message()?;
    let params = serde_json::from_value::<Params>(message.params)?;
    let request = Request::new_v2(message.method, Some(params), Some(Id::Number(1)));
    Ok(format!("{}\n", serde_json::to_string(&request)?))
}

fn assert_initialize_response(line: &str) -> TestResult {
    let response: serde_json::Value = serde_json::from_str(line)?;
    assert_eq!(response["id"], serde_json::json!(1), "response should echo the request id: {response}");
    assert!(response.get("result").is_some(), "initialize should return a result: {response}");
    Ok(())
}
