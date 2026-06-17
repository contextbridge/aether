use super::docker::{Docker, DockerExecConfig};
use super::transcript::is_terminal;
use super::{Agent, AgentConfig, DockerError, DockerImage, RunError, build_task_prompt};
use aether_core::events::AgentMessage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use testcontainers::core::Mount;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::mpsc::Sender;

/// Environment variable through which `DockerAgent` hands the wrapped task prompt to the
/// in-container command.
pub const AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV: &str = "AETHER_EVAL_WRAPPED_TASK_PROMPT";
pub const AETHER_EVAL_WORKSPACE_ROOT_ENV: &str = "AETHER_EVAL_WORKSPACE_ROOT";
pub const AETHER_EVAL_CWD_ENV: &str = "AETHER_EVAL_CWD";

pub struct DockerAgent {
    image: DockerImage,
    command: Vec<String>,
    env_vars: HashMap<String, String>,
    mounts: Vec<Mount>,
    ephemeral_mounts: Vec<String>,
}

impl DockerAgent {
    /// `command` is the in-container argv to exec; its stdout must be newline-delimited
    /// `AgentMessage` JSON.
    pub fn new(image: DockerImage, command: Vec<String>) -> Self {
        Self { image, command, env_vars: HashMap::new(), mounts: Vec::new(), ephemeral_mounts: Vec::new() }
    }

    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    pub fn with_env_vars(mut self, env_vars: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    pub fn with_mount(mut self, mount: Mount) -> Self {
        self.mounts.push(mount);
        self
    }

    /// Register a container path to be backed by a fresh host tempdir, created at run time.
    pub fn with_ephemeral_mount(mut self, container_path: impl Into<String>) -> Self {
        self.ephemeral_mounts.push(container_path.into());
        self
    }
}

impl Agent for DockerAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentMessage>) -> Result<(), RunError> {
        let container_workspace_root = Path::new("/workspace");
        let container_cwd = container_cwd(container_workspace_root, config.relative_cwd);
        let wrapped_task_prompt = build_task_prompt(config.task_prompt, &container_cwd);
        let command = wrap_command(&container_cwd, &self.command);

        let mut docker = Docker::new(self.image.clone());

        for mount in &self.mounts {
            docker = docker.with_mount(mount.clone());
        }

        for (key, value) in &self.env_vars {
            docker = docker.with_env_var(key.clone(), value.clone());
        }

        docker = docker
            .with_env_var(AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, wrapped_task_prompt)
            .with_env_var(AETHER_EVAL_WORKSPACE_ROOT_ENV, container_workspace_root.display().to_string())
            .with_env_var(AETHER_EVAL_CWD_ENV, container_cwd.display().to_string());

        let mut ephemeral_tempdirs = Vec::with_capacity(self.ephemeral_mounts.len());
        for container_path in &self.ephemeral_mounts {
            let tempdir = tempdir().map_err(|source| DockerError::EphemeralMountTempDir { source })?;
            docker = docker.with_mount(Mount::bind_mount(tempdir.path().display().to_string(), container_path.clone()));
            ephemeral_tempdirs.push(tempdir);
        }

        let (sent_terminal, stderr) = {
            let mut session = docker.exec(DockerExecConfig { workspace_root: config.workspace_root, command }).await?;
            let sent_terminal = forward_stdout_messages(session.stdout(), &tx).await?;
            (sent_terminal, session.stderr_to_string().await?)
        };

        if !stderr.trim().is_empty() {
            tracing::debug!("docker agent stderr: {}", stderr.trim());
        }

        if sent_terminal { Ok(()) } else { Err(DockerError::CommandExitWithoutTerminal { stderr }.into()) }
    }
}

async fn forward_stdout_messages<T: AsyncBufRead + Unpin>(
    reader: T,
    tx: &Sender<AgentMessage>,
) -> Result<bool, RunError> {
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await.map_err(|source| DockerError::StdoutRead { source })? {
        if line.trim().is_empty() {
            continue;
        }
        tracing::debug!("docker agent stdout: {line}");
        let message: AgentMessage =
            serde_json::from_str(&line).map_err(|source| DockerError::AgentMessageJsonLine { line, source })?;
        let terminal = is_terminal(&message);
        tx.send(message).await.map_err(|error| RunError::ChannelSendFailed(error.to_string()))?;
        if terminal {
            return Ok(true);
        }
    }

    Ok(false)
}

fn container_cwd(container_workspace_root: &Path, relative_cwd: Option<&Path>) -> PathBuf {
    relative_cwd.map_or_else(
        || container_workspace_root.to_path_buf(),
        |relative_cwd| container_workspace_root.join(relative_cwd),
    )
}

/// Wrap the agent argv in a shell that `cd`s into the eval cwd first. `testcontainers` execs inherit
/// the image working dir (`/workspace`) and expose no per-exec cwd, so the cwd is passed as the
/// shell's first positional (`$1`) and dropped with `shift` before exec-ing the real command. `$0`
/// is a throwaway name.
fn wrap_command(container_cwd: &Path, command: &[String]) -> Vec<String> {
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "cd \"$1\" && shift && exec \"$@\"".to_string(),
        "aether-eval-command".to_string(),
        container_cwd.display().to_string(),
    ];
    argv.extend(command.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_env_vars_merges_explicit_container_env() {
        let agent = DockerAgent::new(DockerImage::new("sandbox", "latest"), vec!["agent".to_string()])
            .with_env_var("ONE", "1")
            .with_env_vars(HashMap::from([("TWO".to_string(), "2".to_string())]));

        assert_eq!(agent.env_vars.get("ONE"), Some(&"1".to_string()));
        assert_eq!(agent.env_vars.get("TWO"), Some(&"2".to_string()));
    }

    #[test]
    fn container_cwd_uses_workspace_root_when_no_relative_cwd() {
        assert_eq!(container_cwd(Path::new("/workspace"), None), Path::new("/workspace"));
    }

    #[test]
    fn container_cwd_joins_relative_cwd() {
        assert_eq!(container_cwd(Path::new("/workspace"), Some(Path::new("subdir"))), Path::new("/workspace/subdir"));
    }

    #[test]
    fn wrap_command_cds_to_container_cwd_then_execs_command() {
        let argv =
            wrap_command(Path::new("/workspace/subdir"), &["node".to_string(), "/app/eval-agent.js".to_string()]);

        assert_eq!(
            argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "cd \"$1\" && shift && exec \"$@\"".to_string(),
                "aether-eval-command".to_string(),
                "/workspace/subdir".to_string(),
                "node".to_string(),
                "/app/eval-agent.js".to_string(),
            ]
        );
    }

    #[test]
    fn wrap_command_preserves_command_args_after_cwd_arg() {
        let argv = wrap_command(
            Path::new("/workspace"),
            &["node".to_string(), "/app/eval agent.js".to_string(), "--city".to_string(), "New York".to_string()],
        );

        assert_eq!(&argv[5..], ["node", "/app/eval agent.js", "--city", "New York"]);
    }
}
