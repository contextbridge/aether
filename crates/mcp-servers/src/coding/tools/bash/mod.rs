use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

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
    pub killed: bool,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub fn validate_args(args: &BashInput) -> Result<(), BashError> {
    if args.command.trim() == "rm" {
        return Err(BashError::Forbidden("Deleting files with a bare 'rm' is not allowed".to_string()));
    }

    if args.timeout.is_some_and(|timeout_ms| timeout_ms > 600_000) {
        return Err(BashError::TimeoutTooLarge);
    }

    Ok(())
}

pub async fn execute_command(args: BashInput) -> Result<BashOutput, BashError> {
    execute_command_in_dir(args, None).await
}

/// Runs `args.command` to completion; `args.timeout: None` means no timeout.
pub async fn execute_command_in_dir(args: BashInput, cwd: Option<&Path>) -> Result<BashOutput, BashError> {
    validate_args(&args)?;

    let timeout = args.timeout.map(Duration::from_millis);

    let mut command = Command::new("bash");
    command.arg("-c").arg(&args.command);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command.kill_on_drop(true);

    let output = match timeout {
        Some(timeout) => {
            let Ok(result) = tokio::time::timeout(timeout, command.output()).await else {
                let display_meta = ToolDisplayMeta::new(
                    "Run command",
                    format!("{} (exit -1, timed out)", truncate(&args.command, 40)),
                );
                return Ok(BashOutput {
                    output: format!("Command timed out after {}ms", timeout.as_millis()),
                    exit_code: -1,
                    killed: true,
                    meta: Some(display_meta.into()),
                });
            };
            result
        }
        None => command.output().await,
    }
    .map_err(|error| BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);
    let display_meta =
        ToolDisplayMeta::new("Run command", format!("{} (exit {exit_code})", truncate(&args.command, 40)));

    Ok(BashOutput { output: format!("{stdout}{stderr}"), exit_code, killed: false, meta: Some(display_meta.into()) })
}
