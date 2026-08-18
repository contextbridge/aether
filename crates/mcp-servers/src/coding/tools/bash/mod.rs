use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::env::current_exe;
use std::os::unix::process::CommandExt;
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
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

/// Immutable environment overrides inherited by Bash commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BashEnvironment {
    vars: BTreeMap<String, String>,
}

impl BashEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(name.into(), value.into());
        self
    }

    pub fn with_path_prepend(self, directory: impl AsRef<Path>) -> Self {
        let directory = directory.as_ref().to_string_lossy().into_owned();
        let current = std::env::var("PATH").unwrap_or_default();
        let mut entries = vec![directory.clone()];
        entries.extend(current.split(':').filter(|entry| *entry != directory).map(str::to_owned));
        self.with_var("PATH", entries.join(":"))
    }

    pub fn with_current_exe_dir_on_path(self) -> Self {
        let Some(directory) = current_exe().ok().and_then(|path| path.parent().map(Path::to_path_buf)) else {
            return self;
        };
        self.with_path_prepend(directory)
    }

    pub fn vars(&self) -> &BTreeMap<String, String> {
        &self.vars
    }
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

/// Runs `args.command` to completion; `args.timeout: None` means no timeout.
pub async fn execute_command(
    args: BashInput,
    cwd: Option<&Path>,
    environment: &BashEnvironment,
) -> Result<BashOutput, BashError> {
    validate_args(&args)?;

    let timeout = args.timeout.map(Duration::from_millis);

    let mut command = Command::new("bash");
    command.arg("-c").arg(&args.command);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.envs(environment.vars());
    command.as_std_mut().process_group(0);

    let child = command
        .spawn()
        .map_err(|error| BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() })?;
    let mut process_group = ProcessGroupGuard::new(child.id());
    let mut child_task = tokio::spawn(child.wait_with_output());

    let output = match timeout {
        Some(timeout) => {
            tokio::select! {
                result = &mut child_task => result,
                () = tokio::time::sleep(timeout) => {
                    process_group.kill();
                    let _ = tokio::time::timeout(Duration::from_secs(5), &mut child_task).await;
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
                }
            }
        }
        None => child_task.await,
    };
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Err(BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() });
        }
    };
    let output =
        output.map_err(|error| BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() })?;
    process_group.disarm();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);
    let display_meta =
        ToolDisplayMeta::new("Run command", format!("{} (exit {exit_code})", truncate(&args.command, 40)));

    Ok(BashOutput { output: format!("{stdout}{stderr}"), exit_code, killed: false, meta: Some(display_meta.into()) })
}

/// Kills an entire process group on drop unless disarmed after a clean exit.
///
/// `Child::kill`/`kill_on_drop` signal only the direct child (`bash`), leaving
/// its spawned commands orphaned; killing the process group covers the tree.
/// Disarming matters because the kernel may recycle the group id once reaped.
struct ProcessGroupGuard {
    pgid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pgid: Option<u32>) -> Self {
        Self { pgid }
    }

    fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            unsafe {
                libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn environment_overrides_are_passed_to_bash() {
        let environment = BashEnvironment::new().with_var("AETHER_TEST_VALUE", "present");
        let output = execute_command(
            BashInput { command: "printf '%s' \"$AETHER_TEST_VALUE\"".into(), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();
        assert_eq!(output.output, "present");
    }
}
