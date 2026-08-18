//! Tokio-native parent-side ACP transport.
//!
//! `agent_client_protocol::AcpAgent` spawns the child via smol's
//! `async_process::Command`, which wraps stdio in `blocking::Unblock`. Inside a
//! tokio runtime that causes a busy loop. This avoids the issue by spawning stdio agents with `tokio::process::Command`

use agent_client_protocol::util::internal_error;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, ByteStreams, ConnectTo, Error, Role, util};
use std::path::PathBuf;
use std::process::Stdio;
use std::str::FromStr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
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
        match child.wait().await {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                let stderr = stderr_rx.await.unwrap_or_default();
                Err(util::internal_error(format!("agent process exited ({s}): {stderr}")))
            }
            Err(e) => Err(Error::into_internal_error(e)),
        }
    };

    let bytes = ByteStreams::new(stdin.compat_write(), stdout.compat());
    tokio::pin!(child_fut);
    tokio::select! {
        result = &mut child_fut => result,
        result = ConnectTo::<T>::connect_to(bytes, client) => {
            result?;
            tokio::time::timeout(Duration::from_secs(1), &mut child_fut)
                .await
                .unwrap_or(Ok(()))
        }
    }
}
