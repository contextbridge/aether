use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::process::Command;

use crate::coding::error::BashError;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, truncate};

/// Environment overrides applied to commands launched by the bash tool.
#[derive(Debug, Clone, Default)]
pub struct BashEnvironment {
    overrides: Arc<RwLock<Vec<(OsString, OsString)>>>,
}

impl BashEnvironment {
    pub fn new(overrides: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>) -> Self {
        Self {
            overrides: Arc::new(RwLock::new(
                overrides.into_iter().map(|(key, value)| (key.into(), value.into())).collect(),
            )),
        }
    }

    pub fn for_aether_executable(executable: Option<&Path>) -> Self {
        let Some(executable_dir) = executable.and_then(Path::parent) else {
            return Self::default();
        };
        let mut paths = vec![PathBuf::from(executable_dir)];
        paths.extend(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|path| path != executable_dir),
        );
        std::env::join_paths(paths).map_or_else(|_| Self::default(), |path| Self::new([("PATH", path)]))
    }

    pub fn extend(&self, overrides: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>) {
        self.overrides
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(overrides.into_iter().map(|(key, value)| (key.into(), value.into())));
    }

    fn entries(&self) -> Vec<(OsString, OsString)> {
        self.overrides.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

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

pub async fn execute_command_in_dir(args: BashInput, cwd: Option<&Path>) -> Result<BashOutput, BashError> {
    execute_command_in_dir_with_env(args, cwd, &BashEnvironment::default()).await
}

pub async fn execute_command_in_dir_with_env(
    args: BashInput,
    cwd: Option<&Path>,
    environment: &BashEnvironment,
) -> Result<BashOutput, BashError> {
    validate_args(&args)?;
    let run = run_bash(&args.command, cwd, environment);
    let result = match args.timeout.map(Duration::from_millis) {
        Some(duration) => tokio::time::timeout(duration, run).await.unwrap_or_else(|_| {
            Ok(ProcessOutput {
                output: format!("Command timed out after {}ms", duration.as_millis()),
                exit_code: -1,
                killed: true,
            })
        })?,
        None => run.await?,
    };
    Ok(bash_output(&args.command, result))
}

struct ProcessOutput {
    output: String,
    exit_code: i32,
    killed: bool,
}

fn bash_output(command: &str, result: ProcessOutput) -> BashOutput {
    let timed_out = if result.killed { ", timed out" } else { "" };
    let display_meta = ToolDisplayMeta::new(
        "Run command",
        format!("{} (exit {}{timed_out})", truncate(command, 40), result.exit_code),
    );
    BashOutput {
        output: result.output,
        exit_code: result.exit_code,
        killed: result.killed,
        meta: Some(display_meta.into()),
    }
}

async fn run_bash(
    command_text: &str,
    cwd: Option<&Path>,
    environment: &BashEnvironment,
) -> Result<ProcessOutput, BashError> {
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(command_text)
        .envs(environment.entries())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    command.process_group(0);
    let child = command
        .spawn()
        .map_err(|error| BashError::SpawnFailed { command: command_text.to_string(), reason: error.to_string() })?;

    let mut process_group = ProcessGroupGuard(child.id().and_then(|id| i32::try_from(id).ok()).map(Pid::from_raw));
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| BashError::CaptureFailed { command: command_text.to_string(), reason: error.to_string() })?;

    process_group.0 = None;
    Ok(ProcessOutput {
        output: format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
        exit_code: output.status.code().unwrap_or(-1),
        killed: false,
    })
}

struct ProcessGroupGuard(Option<Pid>);

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(group) = self.0 {
            let _ = killpg(group, Signal::SIGKILL);
        }
    }
}
