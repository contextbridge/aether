use mcp_utils::{
    client::{McpConfig, McpServer, McpServerConfig, ParseError},
    request_context::validate_server_alias,
};
use std::path::{Path, PathBuf};
use utils::variables::Vars;

pub struct RemoteConfig {
    pub root_dir: PathBuf,
    pub servers: Vec<McpServer>,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("private MCP socket failed")]
    Socket(#[from] mcp_utils::tool_gateway::UnixSocketTransportError),
    #[error("workspace or listener I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid MCP configuration")]
    Config(#[source] ParseError),
    #[error("invalid remote configuration: {0}")]
    Invalid(String),
    #[error("invalid built-in server arguments")]
    Builtin(#[source] mcp_servers::error::ServerInitError),
    #[error("MCP connection setup failed")]
    Connection(#[source] mcp_utils::client::McpError),
    #[error("MCP runtime shutdown failed")]
    Shutdown(#[from] tokio::task::JoinError),
}

impl RemoteConfig {
    pub fn load(root: &Path, paths: &[PathBuf]) -> Result<Self, RemoteError> {
        if paths.is_empty() {
            return Err(RemoteError::Invalid("at least one --mcp-config is required".into()));
        }
        Self::new(root, McpConfig::from_json_files(paths).map_err(RemoteError::Config)?, &Vars::new())
    }

    pub fn new(root: &Path, config: McpConfig, vars: &Vars) -> Result<Self, RemoteError> {
        let root_dir = root.canonicalize()?;
        if !root_dir.is_dir() {
            return Err(RemoteError::Invalid("root-dir must be a directory".into()));
        }
        for (name, server) in &config.servers {
            validate_server_alias(name).map_err(|_| RemoteError::Invalid("invalid backend alias".into()))?;
            if !server.defer_tools().is_model_visible() {
                return Err(RemoteError::Invalid(
                    "put deferTools on the harness gateway entry, not the remote deployment".into(),
                ));
            }
            match server {
                McpServerConfig::InMemory(spec) => {
                    if !matches!(name.as_str(), "coding" | "skills" | "review") {
                        return Err(RemoteError::Invalid(
                            "only coding, skills, and review are remote built-in factories".into(),
                        ));
                    }
                    if spec.input.is_some() {
                        return Err(RemoteError::Invalid("remote built-in servers do not accept startup input".into()));
                    }
                }
                McpServerConfig::Remote(spec) if spec.aether_gateway => {
                    return Err(RemoteError::Invalid("remote backends must not be Aether gateways".into()));
                }
                _ => {}
            }
        }
        let vars = vars.clone().with("WORKSPACE", root_dir.to_string_lossy().into_owned());
        let servers = config.into_servers(&vars).map_err(RemoteError::Config)?;
        Ok(Self { root_dir, servers })
    }
}
