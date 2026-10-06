use super::{
    McpError,
    connection::{McpConnectOutcome, McpServerConnection},
    mcp_client::McpClient,
};
use crate::protocol::client_lifecycle_mode;
use rmcp::{serve_client_with_lifecycle, transport::TokioChildProcess};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStderr, Command},
    task::JoinHandle,
};

pub(super) async fn connect_stdio(
    server_name: &str,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    mcp_client: McpClient,
    cwd: PathBuf,
    generation: u64,
) -> McpConnectOutcome {
    let mut cmd = Command::new(&command);
    cmd.args(&args).envs(&env).current_dir(&cwd);

    let (proc, stderr) = match TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn() {
        Ok(parts) => parts,
        Err(e) => return McpConnectOutcome::Failed { error: McpError::SpawnFailed { command, reason: e.to_string() } },
    };
    let stderr_task = stderr.map(|stderr| spawn_stderr_logger(server_name.to_string(), stderr));

    match serve_client_with_lifecycle(mcp_client, proc, client_lifecycle_mode()).await {
        Ok(client) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, stderr_task, generation),
            reauth_config: None,
        },
        Err(e) => {
            if let Some(task) = stderr_task {
                task.abort();
            }
            McpConnectOutcome::Failed { error: McpError::from(e) }
        }
    }
}

fn spawn_stderr_logger(server_name: String, stderr: ChildStderr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => tracing::info!(server = %server_name, stderr = %line, "MCP server stderr"),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(server = %server_name, %error, "failed to read MCP server stderr");
                    break;
                }
            }
        }
    })
}
