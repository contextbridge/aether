#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("Container operation failed: {0}")]
    Testcontainers(#[from] testcontainers::TestcontainersError),

    #[error("Failed to create ephemeral mount temporary directory: {source}")]
    EphemeralMountTempDir {
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read container stdout: {source}")]
    StdoutRead {
        #[source]
        source: std::io::Error,
    },

    #[error("Container exec completed without an exit code")]
    MissingExecExitCode,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid container image reference '{reference}'")]
pub struct ImageParseError {
    pub(crate) reference: String,
}
