use super::variables::{VarError, expand_env_vars};
use futures::future::BoxFuture;
use rmcp::{RoleServer, service::DynService, transport::streamable_http_client::StreamableHttpClientTransportConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, from_value};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;

/// Top-level MCP configuration
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct RawMcpConfig {
    pub servers: BTreeMap<String, RawMcpServerConfig>,
}

impl<'a> Deserialize<'a> for RawMcpConfig {
    fn deserialize<T: Deserializer<'a>>(deserializer: T) -> Result<Self, T::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(alias = "mcpServers")]
            servers: BTreeMap<String, Value>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let mut servers = BTreeMap::new();

        for (name, mut value) in raw.servers {
            if let Some(map) = value.as_object_mut()
                && !map.contains_key("type")
            {
                map.insert("type".to_string(), Value::String("stdio".to_string()));
            }
            let config: RawMcpServerConfig = from_value(value).map_err(serde::de::Error::custom)?;
            servers.insert(name, config);
        }

        Ok(Self { servers })
    }
}

/// Server connection definition.
///
/// When `"type"` is omitted, defaults to `"stdio"` for compatibility with
/// Claude Code's `.mcp.json` format.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RawMcpServerConfig {
    Stdio {
        command: String,

        #[serde(default)]
        args: Vec<String>,

        #[serde(default)]
        env: HashMap<String, String>,
    },

    Http {
        url: String,

        #[serde(default)]
        headers: HashMap<String, String>,
    },

    Sse {
        url: String,

        #[serde(default)]
        headers: HashMap<String, String>,
    },

    /// In-memory transport (Aether extension) - requires a registered factory
    /// The factory is looked up using the server name from the mcp.json key.
    ///
    /// When `input` contains a `"servers"` key, this is treated as a tool-proxy
    /// configuration: nested server configs are parsed and validated, producing
    /// a `McpServerConfig::ToolProxy` at runtime.
    #[serde(rename = "in-memory")]
    InMemory {
        #[serde(default)]
        args: Vec<String>,

        #[serde(default)]
        input: Option<Value>,
    },
}

/// A single connectable MCP server endpoint.
pub enum ServerConfig {
    Http { name: String, config: StreamableHttpClientTransportConfig },

    Stdio { name: String, command: String, args: Vec<String>, env: HashMap<String, String> },

    InMemory { name: String, server: Box<dyn DynService<RoleServer>> },
}

impl ServerConfig {
    pub fn name(&self) -> &str {
        match self {
            ServerConfig::Http { name, .. }
            | ServerConfig::Stdio { name, .. }
            | ServerConfig::InMemory { name, .. } => name,
        }
    }
}

impl Debug for ServerConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerConfig::Http { name, config } => {
                f.debug_struct("Http").field("name", name).field("config", config).finish()
            }
            ServerConfig::Stdio { name, command, args, env } => f
                .debug_struct("Stdio")
                .field("name", name)
                .field("command", command)
                .field("args", args)
                .field("env", env)
                .finish(),
            ServerConfig::InMemory { name, .. } => {
                f.debug_struct("InMemory").field("name", name).field("server", &"<DynService>").finish()
            }
        }
    }
}

/// Top-level MCP config: a single server OR a tool-proxy of single servers.
pub enum McpServerConfig {
    Server(ServerConfig),
    ToolProxy { name: String, servers: Vec<ServerConfig> },
}

impl McpServerConfig {
    pub fn name(&self) -> &str {
        match self {
            McpServerConfig::Server(cfg) => cfg.name(),
            McpServerConfig::ToolProxy { name, .. } => name,
        }
    }
}

impl From<ServerConfig> for McpServerConfig {
    fn from(cfg: ServerConfig) -> Self {
        McpServerConfig::Server(cfg)
    }
}

impl Debug for McpServerConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            McpServerConfig::Server(cfg) => cfg.fmt(f),
            McpServerConfig::ToolProxy { name, servers } => f
                .debug_struct("ToolProxy")
                .field("name", name)
                .field("servers", &format!("{} nested servers", servers.len()))
                .finish(),
        }
    }
}

/// Factory function that creates an MCP server instance asynchronously.
/// The factory receives parsed CLI arguments and an optional structured input from
/// the `"input"` field in the config JSON.
pub type ServerFactory =
    Box<dyn Fn(Vec<String>, Option<Value>) -> BoxFuture<'static, Box<dyn DynService<RoleServer>>> + Send + Sync>;

#[derive(Debug)]
pub enum ParseError {
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    VarError(VarError),
    FactoryNotFound(String),
    InvalidNestedConfig(String),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::IoError(e) => write!(f, "Failed to read config file: {e}"),
            ParseError::JsonError(e) => write!(f, "Invalid JSON: {e}"),
            ParseError::VarError(e) => write!(f, "Variable expansion failed: {e}"),
            ParseError::FactoryNotFound(name) => {
                write!(f, "InMemory server factory '{name}' not registered")
            }
            ParseError::InvalidNestedConfig(msg) => {
                write!(f, "Invalid nested config in tool-proxy: {msg}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<std::io::Error> for ParseError {
    fn from(error: std::io::Error) -> Self {
        ParseError::IoError(error)
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(error: serde_json::Error) -> Self {
        ParseError::JsonError(error)
    }
}

impl From<VarError> for ParseError {
    fn from(error: VarError) -> Self {
        ParseError::VarError(error)
    }
}

impl RawMcpConfig {
    /// Parse MCP configuration from a JSON file
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content)
    }

    /// Parse and merge multiple MCP config files in order.
    ///
    /// Server name collisions are resolved by "last file wins" via `HashMap::extend`,
    /// so the rightmost file in `paths` takes precedence on overlap.
    pub fn from_json_files<T: AsRef<Path>>(paths: &[T]) -> Result<Self, ParseError> {
        let mut merged = BTreeMap::new();
        for path in paths {
            let raw = Self::from_json_file(path)?;
            merged.extend(raw.servers);
        }
        Ok(Self { servers: merged })
    }

    /// Parse MCP configuration from a JSON string
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Ok(serde_json::from_str(json)?)
    }

    /// Convert to runtime configuration with the provided factory registry
    pub async fn into_configs(
        self,
        factories: &HashMap<String, ServerFactory>,
    ) -> Result<Vec<McpServerConfig>, ParseError> {
        let mut configs = Vec::with_capacity(self.servers.len());
        for (name, raw_config) in self.servers {
            configs.push(raw_config.into_config(name, factories).await?);
        }
        Ok(configs)
    }

    /// Convert all servers to flat `ServerConfig`s suitable for a `ToolProxy`.
    ///
    /// Rejects `in-memory` entries (same restriction as nested tool-proxy servers).
    pub async fn into_proxy_server_configs(
        self,
        factories: &HashMap<String, ServerFactory>,
    ) -> Result<Vec<ServerConfig>, ParseError> {
        let mut configs = Vec::with_capacity(self.servers.len());
        for (name, raw_config) in self.servers {
            if matches!(raw_config, RawMcpServerConfig::InMemory { .. }) {
                return Err(ParseError::InvalidNestedConfig(format!(
                    "in-memory server '{name}' cannot be used inside a proxy-wrapped config file"
                )));
            }
            configs.push(raw_config.into_server_config(name, factories).await?);
        }
        Ok(configs)
    }
}

impl RawMcpServerConfig {
    /// Convert to runtime configuration with the provided factory registry
    pub async fn into_config(
        self,
        name: String,
        factories: &HashMap<String, ServerFactory>,
    ) -> Result<McpServerConfig, ParseError> {
        match self {
            RawMcpServerConfig::Stdio { command, args, env } => Ok(ServerConfig::Stdio {
                name,
                command: expand_env_vars(&command)?,
                args: args.into_iter().map(|a| expand_env_vars(&a)).collect::<Result<Vec<_>, _>>()?,
                env: env
                    .into_iter()
                    .map(|(k, v)| Ok((k, expand_env_vars(&v)?)))
                    .collect::<Result<HashMap<_, _>, VarError>>()?,
            }
            .into()),

            RawMcpServerConfig::Http { url, headers } | RawMcpServerConfig::Sse { url, headers } => {
                // Extract Authorization header if present (only header currently supported)
                let auth_header = headers.get("Authorization").map(|v| expand_env_vars(v)).transpose()?;

                let mut config = StreamableHttpClientTransportConfig::with_uri(expand_env_vars(&url)?);
                if let Some(auth) = auth_header {
                    config = config.auth_header(auth);
                }
                Ok(ServerConfig::Http { name, config }.into())
            }

            RawMcpServerConfig::InMemory { args, input } => {
                let servers_val = input.as_ref().and_then(|v| v.get("servers"));

                if let Some(servers_val) = servers_val {
                    return parse_tool_proxy(name, servers_val, factories).await;
                }

                let server_factory = factories.get(&name).ok_or_else(|| ParseError::FactoryNotFound(name.clone()))?;

                let expanded_args =
                    args.into_iter().map(|a| expand_env_vars(&a)).collect::<Result<Vec<_>, VarError>>()?;

                let server = server_factory(expanded_args, input).await;
                Ok(ServerConfig::InMemory { name, server }.into())
            }
        }
    }

    /// Convert to a `ServerConfig` (non-composite). Used by `parse_tool_proxy`
    /// where the result must be a single server, not a top-level `McpServerConfig`.
    async fn into_server_config(
        self,
        name: String,
        factories: &HashMap<String, ServerFactory>,
    ) -> Result<ServerConfig, ParseError> {
        match self.into_config(name, factories).await? {
            McpServerConfig::Server(cfg) => Ok(cfg),
            McpServerConfig::ToolProxy { name, .. } => Err(ParseError::InvalidNestedConfig(format!(
                "tool-proxy '{name}' cannot be nested inside another tool-proxy"
            ))),
        }
    }
}

async fn parse_tool_proxy(
    name: String,
    servers_val: &Value,
    factories: &HashMap<String, ServerFactory>,
) -> Result<McpServerConfig, ParseError> {
    let nested_raw: HashMap<String, RawMcpServerConfig> = serde_json::from_value(servers_val.clone())
        .map_err(|e| ParseError::InvalidNestedConfig(format!("failed to parse input.servers: {e}")))?;

    let mut nested_configs = Vec::with_capacity(nested_raw.len());
    for (nested_name, nested_raw_cfg) in nested_raw {
        if matches!(nested_raw_cfg, RawMcpServerConfig::InMemory { .. }) {
            return Err(ParseError::InvalidNestedConfig(format!(
                "in-memory servers cannot be nested inside tool-proxy (server: '{nested_name}')"
            )));
        }

        nested_configs.push(Box::pin(nested_raw_cfg.into_server_config(nested_name, factories)).await?);
    }

    Ok(McpServerConfig::ToolProxy { name, servers: nested_configs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_config(dir: &Path, name: &str, json: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, json).unwrap();
        path
    }

    fn stdio_config(command: &str) -> String {
        format!(r#"{{"servers": {{"coding": {{"type": "stdio", "command": "{command}"}}}}}}"#)
    }

    #[test]
    fn from_json_accepts_mcp_servers_key() {
        let config =
            RawMcpConfig::from_json(r#"{"mcpServers": {"alpha": {"type": "stdio", "command": "a"}}}"#).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers.contains_key("alpha"));
    }

    #[test]
    fn from_json_defaults_missing_type_to_stdio() {
        let config = RawMcpConfig::from_json(
            r#"{"mcpServers": {"devtools": {"command": "npx", "args": ["-y", "chrome-devtools-mcp"]}}}"#,
        )
        .unwrap();
        match config.servers.get("devtools").unwrap() {
            RawMcpServerConfig::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "chrome-devtools-mcp"]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn from_json_files_empty_returns_empty_servers() {
        let result = RawMcpConfig::from_json_files::<&str>(&[]).unwrap();
        assert!(result.servers.is_empty());
    }

    #[test]
    fn from_json_files_single_file_matches_from_json_file() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "a.json", &stdio_config("ls"));

        let single = RawMcpConfig::from_json_file(&path).unwrap();
        let multi = RawMcpConfig::from_json_files(&[&path]).unwrap();

        assert_eq!(single.servers.len(), multi.servers.len());
        assert!(multi.servers.contains_key("coding"));
    }

    #[test]
    fn from_json_files_merges_disjoint_servers() {
        let dir = tempdir().unwrap();
        let a = write_config(dir.path(), "a.json", r#"{"servers": {"alpha": {"type": "stdio", "command": "a"}}}"#);
        let b = write_config(dir.path(), "b.json", r#"{"servers": {"beta": {"type": "stdio", "command": "b"}}}"#);

        let merged = RawMcpConfig::from_json_files(&[a, b]).unwrap();
        assert_eq!(merged.servers.len(), 2);
        assert!(merged.servers.contains_key("alpha"));
        assert!(merged.servers.contains_key("beta"));
    }

    #[test]
    fn from_json_files_last_file_wins_on_collision() {
        let dir = tempdir().unwrap();
        let a = write_config(dir.path(), "a.json", &stdio_config("from_a"));
        let b = write_config(dir.path(), "b.json", &stdio_config("from_b"));

        let merged_ab = RawMcpConfig::from_json_files(&[&a, &b]).unwrap();
        match merged_ab.servers.get("coding").unwrap() {
            RawMcpServerConfig::Stdio { command, .. } => assert_eq!(command, "from_b"),
            other => panic!("expected Stdio, got {other:?}"),
        }

        let merged_ba = RawMcpConfig::from_json_files(&[&b, &a]).unwrap();
        match merged_ba.servers.get("coding").unwrap() {
            RawMcpServerConfig::Stdio { command, .. } => assert_eq!(command, "from_a"),
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn from_json_files_propagates_io_error_on_missing_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        let result = RawMcpConfig::from_json_files(&[missing]);
        assert!(matches!(result, Err(ParseError::IoError(_))));
    }

    #[test]
    fn from_json_files_propagates_json_error_on_invalid_file() {
        let dir = tempdir().unwrap();
        let bad = write_config(dir.path(), "bad.json", "not valid json");
        let result = RawMcpConfig::from_json_files(&[bad]);
        assert!(matches!(result, Err(ParseError::JsonError(_))));
    }

    #[tokio::test]
    async fn into_proxy_server_configs_converts_stdio() {
        let config = RawMcpConfig::from_json(
            r#"{"servers": {"alpha": {"type": "stdio", "command": "a"}, "beta": {"type": "stdio", "command": "b"}}}"#,
        )
        .unwrap();

        let factories = HashMap::new();
        let configs = config.into_proxy_server_configs(&factories).await.unwrap();
        assert_eq!(configs.len(), 2);
        let names: Vec<&str> = configs.iter().map(ServerConfig::name).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[tokio::test]
    async fn into_proxy_server_configs_rejects_in_memory() {
        let config = RawMcpConfig::from_json(r#"{"servers": {"bad": {"type": "in-memory"}}}"#).unwrap();

        let factories = HashMap::new();
        let result = config.into_proxy_server_configs(&factories).await;
        assert!(matches!(result, Err(ParseError::InvalidNestedConfig(_))));
    }
}
