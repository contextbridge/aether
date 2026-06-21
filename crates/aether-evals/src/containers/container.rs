use crate::Workspace;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use testcontainers::core::{ExecCommand, ExecResult, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::AsyncBufRead;

use super::{ContainerError, Image};

#[derive(Clone)]
pub struct Container {
    inner: Arc<ContainerInner>,
}

struct ContainerInner {
    container: ContainerAsync<GenericImage>,
    workspace_root: PathBuf,
    cwd: PathBuf,
    _ephemeral_tempdirs: Vec<TempDir>,
}

pub struct ContainerBuilder {
    image: Image,
    env_vars: BTreeMap<String, String>,
    mounts: Vec<Mount>,
    ephemeral_mounts: Vec<String>,
}

pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) struct ExecHandle {
    exec: ExecResult,
}

impl Container {
    pub fn builder(image: Image) -> ContainerBuilder {
        ContainerBuilder { image, env_vars: BTreeMap::new(), mounts: Vec::new(), ephemeral_mounts: Vec::new() }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.inner.workspace_root
    }

    pub fn cwd(&self) -> &Path {
        &self.inner.cwd
    }

    pub async fn exec_shell(&self, script: impl Into<String>) -> Result<ExecOutput, ContainerError> {
        let command = vec!["/bin/sh".to_string(), "-lc".to_string(), script.into()];
        self.exec_streaming(command, BTreeMap::new()).await?.collect().await
    }

    pub(crate) async fn exec_streaming(
        &self,
        command: Vec<String>,
        env_vars: BTreeMap<String, String>,
    ) -> Result<ExecHandle, ContainerError> {
        let exec = self
            .inner
            .container
            .exec(ExecCommand::new(wrap_command(&self.inner.cwd, &command)).with_env_vars(env_vars))
            .await?;
        Ok(ExecHandle { exec })
    }
}

impl ContainerBuilder {
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

    pub fn with_ephemeral_mount(mut self, container_path: impl Into<String>) -> Self {
        self.ephemeral_mounts.push(container_path.into());
        self
    }

    pub async fn start(self, workspace: &Workspace) -> Result<Container, ContainerError> {
        let container_workspace_root = PathBuf::from("/workspace");
        let container_cwd = container_cwd(&container_workspace_root, workspace.relative_cwd());
        let mut image = GenericImage::new(&self.image.name, &self.image.tag)
            .with_entrypoint("/bin/sh")
            .with_cmd(["-c", "sleep infinity"])
            .with_mount(Mount::bind_mount(workspace.root_path().display().to_string(), "/workspace"))
            .with_working_dir(container_cwd.display().to_string());

        for mount in &self.mounts {
            image = image.with_mount(mount.clone());
        }

        for (key, value) in &self.env_vars {
            image = image.with_env_var(key.clone(), value.clone());
        }

        let mut ephemeral_tempdirs = Vec::with_capacity(self.ephemeral_mounts.len());
        for container_path in &self.ephemeral_mounts {
            let tempdir = tempdir().map_err(|source| ContainerError::EphemeralMountTempDir { source })?;
            image = image.with_mount(Mount::bind_mount(tempdir.path().display().to_string(), container_path.clone()));
            ephemeral_tempdirs.push(tempdir);
        }

        let container = image.start().await?;
        Ok(Container {
            inner: Arc::new(ContainerInner {
                container,
                workspace_root: container_workspace_root,
                cwd: container_cwd,
                _ephemeral_tempdirs: ephemeral_tempdirs,
            }),
        })
    }
}

impl ExecHandle {
    pub fn stdout(&mut self) -> impl AsyncBufRead + Send + '_ {
        self.exec.stdout()
    }

    pub async fn stderr_to_string(&mut self) -> Result<String, ContainerError> {
        let stderr = self.exec.stderr_to_vec().await?;
        Ok(String::from_utf8_lossy(&stderr).into_owned())
    }

    pub async fn collect(mut self) -> Result<ExecOutput, ContainerError> {
        let stdout = String::from_utf8_lossy(&self.exec.stdout_to_vec().await?).into_owned();
        let stderr = String::from_utf8_lossy(&self.exec.stderr_to_vec().await?).into_owned();
        let exit_code = self.exec.exit_code().await?.ok_or(ContainerError::MissingExecExitCode)?;
        Ok(ExecOutput { exit_code, stdout, stderr })
    }
}

fn container_cwd(container_workspace_root: &Path, relative_cwd: Option<&Path>) -> PathBuf {
    relative_cwd.map_or_else(
        || container_workspace_root.to_path_buf(),
        |relative_cwd| container_workspace_root.join(relative_cwd),
    )
}

fn wrap_command(container_cwd: &Path, command: &[String]) -> Vec<String> {
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "cd \"$1\" && shift && exec \"$@\"".to_string(),
        "aether-container-exec".to_string(),
        container_cwd.display().to_string(),
    ];
    argv.extend(command.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "aether-container-exec".to_string(),
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
