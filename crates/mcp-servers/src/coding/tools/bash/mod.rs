use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::coding::error::BashError;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, truncate};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BashInput {
    /// The command to execute
    pub command: String,
    /// Optional timeout in milliseconds (max 600000)
    pub timeout: Option<u64>,
    /// Clear, concise description of what this command does in 5-10 words
    pub description: Option<String>,
    /// Set to true to run this command in the background
    #[serde(alias = "run_in_background")]
    pub run_in_background: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BashOutput {
    /// Combined stdout and stderr output
    pub output: String,
    /// Exit code of the command
    pub exit_code: i32,
    /// Whether the command was killed due to timeout
    pub killed: Option<bool>,
    /// Shell ID for background processes
    pub shell_id: Option<String>,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug)]
pub struct BackgroundProcessHandle {
    pub shell_id: String,
    pub output_rx: mpsc::UnboundedReceiver<String>,
    pub task_handle: JoinHandle<(i32, bool)>,
}

#[derive(Debug)]
pub enum BashResult {
    Completed(BashOutput),
    Background(BackgroundProcessHandle),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadBackgroundBashInput {
    /// The ID of the background shell to retrieve output from
    #[serde(alias = "bash_id")]
    pub bash_id: String,
    /// Optional regex to filter output lines
    pub filter: Option<String>,
}

/// Status of a background shell process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundShellStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadBackgroundBashOutput {
    /// New output since last check
    pub output: String,
    /// Current shell status
    pub status: BackgroundShellStatus,
    /// Exit code (when completed)
    pub exit_code: Option<i32>,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn read_background_bash(
    handle: BackgroundProcessHandle,
    filter: Option<String>,
) -> Result<(ReadBackgroundBashOutput, Option<BackgroundProcessHandle>), BashError> {
    let BackgroundProcessHandle { shell_id, mut output_rx, task_handle } = handle;

    // Collect all available output
    let mut output = String::new();
    let filter_regex = if let Some(pattern) = filter {
        Some(regex::Regex::new(&pattern).map_err(|e| BashError::InvalidRegex(e.to_string()))?)
    } else {
        None
    };

    while let Ok(line) = output_rx.try_recv() {
        if let Some(ref regex) = filter_regex {
            if regex.is_match(&line) {
                output.push_str(&line);
            }
        } else {
            output.push_str(&line);
        }
    }

    if task_handle.is_finished() {
        let (exit_code, killed) = task_handle.await.map_err(|e| BashError::JoinFailed(e.to_string()))?;

        let status = if killed { BackgroundShellStatus::Failed } else { BackgroundShellStatus::Completed };

        let display_meta = ToolDisplayMeta::new("Run command", format!("<background> (exit {exit_code})"));

        Ok((
            ReadBackgroundBashOutput { output, status, exit_code: Some(exit_code), meta: Some(display_meta.into()) },
            None,
        ))
    } else {
        let display_meta = ToolDisplayMeta::new("Run command", "<background> (running)");

        Ok((
            ReadBackgroundBashOutput {
                output,
                status: BackgroundShellStatus::Running,
                exit_code: None,
                meta: Some(display_meta.into()),
            },
            Some(BackgroundProcessHandle { shell_id, output_rx, task_handle }),
        ))
    }
}

async fn run_command_with_timeout(
    command: String,
    timeout: Option<Duration>,
    output_tx: mpsc::UnboundedSender<String>,
    cwd: Option<&std::path::Path>,
) -> (i32, bool) {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(&command);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let tx_clone = output_tx.clone();
    let run_command = async move {
        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {e}"))?;

        if let Some(stdout) = child.stdout.take() {
            let tx = tx_clone.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line + "\n");
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let tx = tx_clone.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line + "\n");
                }
            });
        }

        let status = child.wait().await.map_err(|e| format!("Wait failed: {e}"))?;
        Ok::<_, String>((status.code().unwrap_or(-1), false))
    };

    if let Some(timeout_duration) = timeout {
        tokio::time::timeout(timeout_duration, run_command).await.map_or_else(
            |_| {
                let _ = output_tx.send("Command timed out\n".to_string());
                (-1, true)
            },
            |inner| inner.unwrap_or((-1, false)),
        )
    } else {
        run_command.await.unwrap_or((-1, false))
    }
}

pub async fn execute_command(args: BashInput) -> Result<BashResult, BashError> {
    execute_command_in_dir(args, None).await
}

pub async fn execute_command_in_dir(args: BashInput, cwd: Option<&std::path::Path>) -> Result<BashResult, BashError> {
    if args.command.trim() == "rm" {
        return Err(BashError::Forbidden("No you can't fucking delete files".to_string()));
    }

    // Validate timeout is within bounds
    if let Some(timeout_ms) = args.timeout
        && timeout_ms > 600_000
    {
        return Err(BashError::TimeoutTooLarge);
    }

    let run_in_background = args.run_in_background.unwrap_or(false);
    let timeout_duration = args.timeout.map(Duration::from_millis);

    if run_in_background {
        let shell_id = Uuid::new_v4().to_string();
        let command = args.command.clone();
        let cwd = cwd.map(Path::to_path_buf);

        let (output_tx, output_rx) = mpsc::unbounded_channel();

        let task_handle = tokio::spawn(async move {
            run_command_with_timeout(command, timeout_duration, output_tx, cwd.as_deref()).await
        });

        Ok(BashResult::Background(BackgroundProcessHandle { shell_id, output_rx, task_handle }))
    } else {
        // Run synchronously with default timeout of 120000ms (2 minutes)
        let timeout = timeout_duration.or(Some(Duration::from_mins(2)));
        let command = args.command.clone();

        // Collect output in-memory for synchronous case
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&command);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let result = if let Some(timeout_duration) = timeout {
            tokio::time::timeout(timeout_duration, cmd.output()).await
        } else {
            Ok(cmd.output().await)
        };

        let Ok(output) = result else {
            let timeout_ms = timeout.map_or(120_000, |d| d.as_millis());
            let display_meta =
                ToolDisplayMeta::new("Run command", format!("{} (exit -1, timed out)", truncate(&args.command, 40)));
            return Ok(BashResult::Completed(BashOutput {
                output: format!("Command timed out after {timeout_ms}ms"),
                exit_code: -1,
                killed: Some(true),
                shell_id: None,
                meta: Some(display_meta.into()),
            }));
        };

        let output = match output {
            Ok(output) => output,
            Err(e) => return Err(BashError::SpawnFailed { command: args.command, reason: e.to_string() }),
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        let combined_output = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{stdout}{stderr}")
        };

        let display_meta =
            ToolDisplayMeta::new("Run command", format!("{} (exit {exit_code})", truncate(&args.command, 40)));

        Ok(BashResult::Completed(BashOutput {
            output: combined_output,
            exit_code,
            killed: Some(false),
            shell_id: None,
            meta: Some(display_meta.into()),
        }))
    }
}
