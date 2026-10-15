use crate::TestResult;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Error as IoError, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// An `aether acp` child process speaking newline-delimited JSON-RPC over piped stdio.
pub struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

pub struct AcpProcessBuilder {
    command: Command,
}

impl AcpProcess {
    pub fn builder() -> AcpProcessBuilder {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
        command.arg("acp").stderr(Stdio::null());
        AcpProcessBuilder { command }
    }

    pub fn initialize(&mut self) -> TestResult<Value> {
        self.request("initialize", &initialize_params()?)
    }

    /// Send a request and return its `result`, skipping unrelated messages.
    pub fn request(&mut self, method: &str, params: &Value) -> TestResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;

        loop {
            let mut message = self.read_message()?;
            if message["id"] != json!(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(IoError::other(format!("{method} failed: {error}")).into());
            }
            return Ok(message["result"].take());
        }
    }

    pub fn send(&mut self, message: &Value) -> TestResult {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn read_message(&mut self) -> TestResult<Value> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(IoError::other("agent exited before sending a message").into());
        }
        Ok(serde_json::from_str(&line)?)
    }
}

pub fn initialize_params() -> TestResult<Value> {
    Ok(serde_json::to_value(InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test", "1")))?)
}

impl AcpProcessBuilder {
    pub fn log_dir(mut self, dir: &Path) -> Self {
        self.command.arg("--log-dir").arg(dir);
        self
    }

    pub fn settings_json(self, settings: &str) -> Self {
        self.arg("--settings-json").arg(settings)
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.command.arg(arg);
        self
    }

    pub fn env(mut self, key: &str, value: impl AsRef<OsStr>) -> Self {
        self.command.env(key, value);
        self
    }

    pub fn spawn(mut self) -> TestResult<AcpProcess> {
        let mut child = self.command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| IoError::other("child stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| IoError::other("child stdout"))?;
        Ok(AcpProcess { child, stdin, stdout: BufReader::new(stdout), next_id: 1 })
    }

    /// The configured command, for tests that supply their own stdio.
    pub fn into_command(self) -> Command {
        self.command
    }
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
