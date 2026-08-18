//! Tokio-native parent-side ACP transport.
//!
//! `agent_client_protocol::AcpAgent` spawns the child via smol's
//! `async_process::Command`, which wraps stdio in `blocking::Unblock`. Inside a
//! tokio runtime that causes a busy loop. This avoids the issue by spawning stdio agents with `tokio::process::Command`

use agent_client_protocol::util::internal_error;
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, ByteStreams, ConnectTo, Error, INCOMING_TRANSPORT_CLOSED_REASON, Role,
    is_incoming_transport_closed,
};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub struct TokioAcpAgent {
    config: AcpAgentConfig,
}

impl TokioAcpAgent {
    pub fn from_command(command: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self { config: AcpAgentConfig::new(command).args(args) }
    }

    pub fn config(&self) -> &AcpAgentConfig {
        &self.config
    }
}

impl<T: Role> ConnectTo<T> for TokioAcpAgent {
    async fn connect_to(self, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
        connect_stdio::<T>(self.config, client).await
    }
}

impl FromStr for TokioAcpAgent {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self { config: AcpAgent::from_str(s)?.into_config() })
    }
}

async fn connect_stdio<T: Role>(config: AcpAgentConfig, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
    let (stdin, stdout, stderr, mut child) = {
        let mut cmd = Command::new(config.command());
        cmd.args(config.arguments());
        for (name, value) in config.environment() {
            cmd.env(name, value);
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::into_internal_error)?;

        let stdin = child.stdin.take().ok_or_else(|| internal_error("missing child stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| internal_error("missing child stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| internal_error("missing child stderr"))?;
        (stdin, stdout, stderr, child)
    };

    let (stderr_tx, stderr_rx) = oneshot::channel::<String>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut buf = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&line);
        }
        let _ = stderr_tx.send(buf);
    });

    let child_fut = async move {
        let status = child.wait().await.map_err(Error::into_internal_error)?;
        finish_child_exit(status, stderr_rx).await
    };

    let bytes = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let protocol_fut = ConnectTo::<T>::connect_to(bytes, client);
    tokio::pin!(child_fut);

    tokio::select! {
        result = &mut child_fut => result,
        result = protocol_fut => match result {
            Ok(()) => timeout(SHUTDOWN_GRACE_PERIOD, &mut child_fut).await.unwrap_or(Ok(())),
            Err(protocol_error) if has_incoming_transport_closed(&protocol_error) => {
                match timeout(SHUTDOWN_GRACE_PERIOD, &mut child_fut).await {
                    Ok(Err(child_error)) => Err(child_error),
                    _ => Err(protocol_error),
                }
            }
            Err(error) => Err(error),
        },
    }
}

fn has_incoming_transport_closed(error: &Error) -> bool {
    fn data_has_reason(data: &serde_json::Value) -> bool {
        data.get("reason").and_then(serde_json::Value::as_str) == Some(INCOMING_TRANSPORT_CLOSED_REASON)
            || data.get("data").is_some_and(data_has_reason)
    }

    is_incoming_transport_closed(error) || error.data.as_ref().is_some_and(data_has_reason)
}

async fn finish_child_exit(status: ExitStatus, stderr_rx: oneshot::Receiver<String>) -> Result<(), Error> {
    if status.success() {
        return Ok(());
    }

    let stderr = match timeout(SHUTDOWN_GRACE_PERIOD, stderr_rx).await {
        Ok(Ok(stderr)) => stderr,
        _ => String::new(),
    };
    let message = if stderr.is_empty() {
        format!("agent process exited ({status})")
    } else {
        format!("agent process exited ({status}): {stderr}")
    };

    Err(internal_error(message))
}

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(1);
