//! Tokio-native parent-side ACP transport.
//!
//! `agent_client_protocol::AcpAgent` spawns the child via smol's
//! `async_process::Command`, which wraps stdio in `blocking::Unblock`. Inside a
//! tokio runtime that causes a busy loop. This avoids the issue by spawning stdio agents with `tokio::process::Command`
//!
use agent_client_protocol::schema::{McpServer, McpServerStdio};
use agent_client_protocol::util::internal_error;
use agent_client_protocol::{AcpAgent, ByteStreams, ConnectTo, Error, Role, util};
use std::pin::pin;
use std::process::Stdio;
use std::str::FromStr;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub struct TokioAcpAgent {
    stdio: McpServerStdio,
}

impl TokioAcpAgent {
    pub fn stdio(&self) -> &McpServerStdio {
        &self.stdio
    }
}

impl<T: Role> ConnectTo<T> for TokioAcpAgent {
    async fn connect_to(self, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
        connect_stdio::<T>(self.stdio, client).await
    }
}

impl FromStr for TokioAcpAgent {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match AcpAgent::from_str(s)?.into_server() {
            McpServer::Stdio(stdio) => Ok(Self { stdio }),
            _ => Err(util::internal_error("unsupported ACP agent transport")),
        }
    }
}

async fn connect_stdio<T: Role>(server: McpServerStdio, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
    let (stdin, stdout, stderr, mut child) = {
        let mut cmd = Command::new(&server.command);
        cmd.args(&server.args);
        for env_var in &server.env {
            cmd.env(&env_var.name, &env_var.value);
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

    let bytes = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let mut wait = pin!(child.wait());
    let mut protocol = pin!(ConnectTo::<T>::connect_to(bytes, client));
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_buf = String::new();
    let mut stderr_open = true;

    loop {
        tokio::select! {
            result = &mut protocol => return result,
            status = &mut wait => {
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    stderr_buf.push_str(&line);
                    stderr_buf.push('\n');
                }

                return match status {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(util::internal_error(format!("agent process exited ({s}): {stderr_buf}"))),
                    Err(e) => Err(Error::into_internal_error(e)),
                };
            }

            line = stderr_lines.next_line(), if stderr_open => match line {
                Ok(Some(line)) => {
                    stderr_buf.push_str(&line);
                    stderr_buf.push('\n');
                }

                Ok(None) | Err(_) => stderr_open = false,
            }
        }
    }
}
