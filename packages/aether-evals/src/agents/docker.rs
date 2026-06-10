use std::path::Path;
use testcontainers::core::{ExecCommand, ExecResult, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::AsyncBufRead;

pub(crate) struct Docker {
    image: DockerImage,
    env_vars: Vec<(String, String)>,
    mounts: Vec<Mount>,
}

pub(crate) struct DockerExecConfig<'a> {
    pub workspace_root: &'a Path,
    pub command: Vec<String>,
}

pub(crate) struct DockerExecSession {
    _container: ContainerAsync<GenericImage>,
    exec: ExecResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerImage {
    pub name: String,
    pub tag: String,
}

impl Docker {
    pub fn new(image: DockerImage) -> Self {
        Self { image, env_vars: Vec::new(), mounts: Vec::new() }
    }

    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }

    pub fn with_mount(mut self, mount: Mount) -> Self {
        self.mounts.push(mount);
        self
    }

    pub async fn exec(&self, config: DockerExecConfig<'_>) -> Result<DockerExecSession, DockerError> {
        let mut image = GenericImage::new(&self.image.name, &self.image.tag)
            .with_entrypoint("/bin/sh")
            .with_cmd(["-c", "sleep infinity"])
            .with_mount(Mount::bind_mount(config.workspace_root.display().to_string(), "/workspace"))
            .with_working_dir("/workspace");

        for mount in &self.mounts {
            image = image.with_mount(mount.clone());
        }

        for (key, value) in &self.env_vars {
            image = image.with_env_var(key.clone(), value.clone());
        }

        let container = image.start().await?;
        let exec = container.exec(ExecCommand::new(config.command)).await?;
        Ok(DockerExecSession { _container: container, exec })
    }
}

impl DockerExecSession {
    pub fn stdout(&mut self) -> impl AsyncBufRead + Send + '_ {
        self.exec.stdout()
    }

    pub async fn stderr_to_string(&mut self) -> Result<String, DockerError> {
        let stderr = self.exec.stderr_to_vec().await?;
        Ok(String::from_utf8_lossy(&stderr).into_owned())
    }
}

impl DockerImage {
    pub fn new(name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self { name: name.into(), tag: tag.into() }
    }

    pub fn parse(reference: &str) -> Result<Self, DockerImageParseError> {
        parse_image_reference(reference).ok_or_else(|| DockerImageParseError { reference: reference.to_string() })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("Docker container operation failed: {0}")]
    Testcontainers(#[from] testcontainers::TestcontainersError),

    #[error("Failed to create Docker AETHER_HOME temporary directory: {source}")]
    AetherHomeTempDir {
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read Docker stdout: {source}")]
    StdoutRead {
        #[source]
        source: std::io::Error,
    },

    #[error("Aether headless exited without completing: {stderr}")]
    HeadlessExit { stderr: String },

    #[error("Aether headless emitted invalid JSON line: {line}")]
    HeadlessJsonLine {
        line: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid Docker image reference '{reference}'")]
pub struct DockerImageParseError {
    pub(crate) reference: String,
}

fn parse_image_reference(reference: &str) -> Option<DockerImage> {
    if reference.is_empty() || reference.starts_with(':') || reference.ends_with(':') {
        return None;
    }

    let last_slash = reference.rfind('/');
    let last_colon = reference.rfind(':');
    match last_colon.filter(|colon| last_slash.is_none_or(|slash| *colon > slash)) {
        Some(colon) => Some(DockerImage::new(&reference[..colon], &reference[colon + 1..])),
        None => Some(DockerImage::new(reference, "latest")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_image_parse_handles_name_tag() {
        assert_eq!(DockerImage::parse("aether-sandbox:dev").unwrap(), DockerImage::new("aether-sandbox", "dev"));
    }

    #[test]
    fn docker_image_parse_handles_registry_path() {
        assert_eq!(
            DockerImage::parse("ghcr.io/org/aether:sha").unwrap(),
            DockerImage::new("ghcr.io/org/aether", "sha")
        );
    }

    #[test]
    fn docker_image_parse_defaults_latest() {
        assert_eq!(DockerImage::parse("aether-sandbox").unwrap(), DockerImage::new("aether-sandbox", "latest"));
    }

    #[test]
    fn docker_image_parse_rejects_invalid_reference() {
        assert!(DockerImage::parse(":latest").is_err());
        assert!(DockerImage::parse("aether:").is_err());
    }
}
