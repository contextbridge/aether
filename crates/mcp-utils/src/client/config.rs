use futures::future::BoxFuture;
use rmcp::{RoleServer, service::DynService, transport::streamable_http_client::StreamableHttpClientTransportConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::num::NonZeroU16;
use std::path::Path;
use utils::is_false;
use utils::variables::{VarError, Vars};

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct McpConfig {
    #[serde(alias = "mcpServers")]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[doc = include_str!("../docs/mcp_server_config.md")]
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum McpServerConfig {
    Stdio(StdioServerConfig),
    Remote(RemoteServerConfig),
    InMemory(InMemoryServerConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StdioServerConfig {
    /// Transport discriminant; always `stdio`.
    #[serde(rename = "type", default)]
    pub type_: StdioType,

    /// Executable launched to run the MCP server over stdio.
    pub command: String,

    /// Command-line arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables set for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Expose this server's tools through Aether's tool proxy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub proxy: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpOAuthConfig {
    pub client_id: String,
    pub callback_port: NonZeroU16,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteServerConfig {
    /// Transport discriminant; `http` (streamable HTTP) or `sse` (Server-Sent Events).
    #[serde(rename = "type")]
    pub type_: RemoteType,

    /// Base URL of the remote MCP server.
    pub url: String,

    /// Extra HTTP headers sent with every request.
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// OAuth settings for a pre-registered public client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,

    /// Expose this server's tools through Aether's tool proxy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub proxy: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InMemoryServerConfig {
    /// Transport discriminant; always `in-memory`.
    #[serde(rename = "type")]
    pub type_: InMemoryType,

    /// Arguments passed to the built-in (in-process) server.
    #[serde(default)]
    pub args: Vec<String>,

    /// Optional JSON input passed to the built-in server at startup.
    #[serde(default)]
    pub input: Option<Value>,

    /// Expose this server's tools through Aether's tool proxy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub proxy: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
pub enum StdioType {
    #[default]
    #[serde(rename = "stdio")]
    Stdio,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq)]
pub enum RemoteType {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "sse")]
    Sse,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq)]
pub enum InMemoryType {
    #[serde(rename = "in-memory")]
    InMemory,
}

pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    pub proxy: bool,
}

pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Http(McpHttpConfig),
    InMemory { server: Box<dyn DynService<RoleServer>> },
}

#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    pub transport: StreamableHttpClientTransportConfig,
    pub oauth: Option<McpOAuthConfig>,
}

impl McpHttpConfig {
    pub fn oauth_client_id(&self) -> Option<&str> {
        self.oauth.as_ref().map(|oauth| oauth.client_id.as_str())
    }

    pub fn callback_port(&self) -> Option<NonZeroU16> {
        self.oauth.as_ref().map(|oauth| oauth.callback_port)
    }
}

impl From<StreamableHttpClientTransportConfig> for McpHttpConfig {
    fn from(transport: StreamableHttpClientTransportConfig) -> Self {
        Self { transport, oauth: None }
    }
}

impl McpServer {
    pub fn new(name: impl Into<String>, transport: McpTransport, proxy: bool) -> Self {
        Self { name: name.into(), transport, proxy }
    }

    /// Clone this server config. Fails for [`McpTransport::InMemory`], whose
    /// boxed service cannot be duplicated and so cannot be shared across
    /// independently-spawned MCP managers.
    pub fn try_clone(&self) -> Result<Self, McpServerCloneError> {
        let transport = match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                McpTransport::Stdio { command: command.clone(), args: args.clone(), env: env.clone() }
            }
            McpTransport::Http(config) => McpTransport::Http(config.clone()),
            McpTransport::InMemory { .. } => return Err(McpServerCloneError(self.name.clone())),
        };
        Ok(Self { name: self.name.clone(), transport, proxy: self.proxy })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("in-memory MCP server `{0}` cannot be cloned across runtimes")]
pub struct McpServerCloneError(pub String);

impl Debug for McpServer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("transport", &self.transport)
            .field("proxy", &self.proxy)
            .finish()
    }
}

impl Debug for McpTransport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            McpTransport::Stdio { command, args, env } => {
                f.debug_struct("Stdio").field("command", command).field("args", args).field("env", env).finish()
            }
            McpTransport::Http(config) => f.debug_tuple("Http").field(config).finish(),
            McpTransport::InMemory { .. } => f.debug_struct("InMemory").field("server", &"<DynService>").finish(),
        }
    }
}

pub type ServerFactory =
    Box<dyn Fn(Vec<String>, Option<Value>) -> BoxFuture<'static, Box<dyn DynService<RoleServer>>> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Variable expansion failed: {0}")]
    VarError(#[from] VarError),

    #[error("InMemory server factory '{0}' not registered")]
    FactoryNotFound(String),

    #[error("Invalid nested config in tool-proxy: {0}")]
    InvalidNestedConfig(String),
}

impl McpConfig {
    pub fn new(servers: BTreeMap<String, McpServerConfig>) -> Self {
        Self { servers }
    }

    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content)
    }

    pub fn from_json_files<T: AsRef<Path>>(paths: &[T]) -> Result<Self, ParseError> {
        let mut merged = BTreeMap::new();
        for path in paths {
            let raw = Self::from_json_file(path)?;
            merged.extend(raw.servers);
        }
        Ok(Self::new(merged))
    }

    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Ok(serde_json::from_str(json)?)
    }

    pub async fn into_servers(
        self,
        factories: &HashMap<String, ServerFactory>,
        vars: &Vars,
    ) -> Result<Vec<McpServer>, ParseError> {
        self.into_servers_with_proxy(factories, vars, false).await
    }

    pub async fn into_servers_with_proxy(
        self,
        factories: &HashMap<String, ServerFactory>,
        vars: &Vars,
        force_proxy: bool,
    ) -> Result<Vec<McpServer>, ParseError> {
        let mut servers = Vec::with_capacity(self.servers.len());
        for (name, config) in self.servers {
            servers.push(config.into_server(name, factories, vars, force_proxy).await?);
        }
        Ok(servers)
    }

    pub fn mark_all_proxy(&mut self) {
        for server in self.servers.values_mut() {
            server.set_proxy(true);
        }
    }
}

impl McpServerConfig {
    pub fn proxy(&self) -> bool {
        match self {
            McpServerConfig::Stdio(config) => config.proxy,
            McpServerConfig::Remote(config) => config.proxy,
            McpServerConfig::InMemory(config) => config.proxy,
        }
    }

    pub fn set_proxy(&mut self, value: bool) {
        match self {
            McpServerConfig::Stdio(config) => config.proxy = value,
            McpServerConfig::Remote(config) => config.proxy = value,
            McpServerConfig::InMemory(config) => config.proxy = value,
        }
    }

    pub async fn into_server(
        self,
        name: String,
        factories: &HashMap<String, ServerFactory>,
        vars: &Vars,
        force_proxy: bool,
    ) -> Result<McpServer, ParseError> {
        let proxy = force_proxy || self.proxy();
        let transport = self.into_transport(name.clone(), factories, vars).await?;
        Ok(McpServer::new(name, transport, proxy))
    }

    async fn into_transport(
        self,
        name: String,
        factories: &HashMap<String, ServerFactory>,
        vars: &Vars,
    ) -> Result<McpTransport, ParseError> {
        match self {
            McpServerConfig::Stdio(StdioServerConfig { command, args, env, .. }) => Ok(McpTransport::Stdio {
                command: vars.expand(&command)?,
                args: args.into_iter().map(|a| vars.expand(&a)).collect::<Result<Vec<_>, _>>()?,
                env: env
                    .into_iter()
                    .map(|(k, v)| Ok((k, vars.expand(&v)?)))
                    .collect::<Result<HashMap<_, _>, VarError>>()?,
            }),

            McpServerConfig::Remote(RemoteServerConfig { url, headers, oauth, .. }) => {
                let auth_header = headers.get("Authorization").map(|v| vars.expand(v)).transpose()?.map(|auth| {
                    // rmcp adds `Bearer`  to the auth header.
                    auth.split_once(' ')
                        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
                        .map_or(auth.as_str(), |(_, rest)| rest)
                        .to_string()
                });

                let mut transport = StreamableHttpClientTransportConfig::with_uri(vars.expand(&url)?);
                if let Some(auth) = auth_header {
                    transport = transport.auth_header(auth);
                }

                let oauth = oauth
                    .map(|oauth| -> Result<McpOAuthConfig, VarError> {
                        Ok(McpOAuthConfig { client_id: vars.expand(&oauth.client_id)?, ..oauth })
                    })
                    .transpose()?;

                Ok(McpTransport::Http(McpHttpConfig { transport, oauth }))
            }

            McpServerConfig::InMemory(InMemoryServerConfig { args, input, .. }) => {
                let server_factory = factories.get(&name).ok_or_else(|| ParseError::FactoryNotFound(name.clone()))?;
                let expanded_args = args.into_iter().map(|a| vars.expand(&a)).collect::<Result<Vec<_>, VarError>>()?;
                let server = server_factory(expanded_args, input).await;
                Ok(McpTransport::InMemory { server })
            }
        }
    }
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
        let config = McpConfig::from_json(r#"{"mcpServers": {"alpha": {"type": "stdio", "command": "a"}}}"#).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers.contains_key("alpha"));
    }

    #[test]
    fn from_json_defaults_missing_type_to_stdio() {
        let config = McpConfig::from_json(
            r#"{"mcpServers": {"devtools": {"command": "npx", "args": ["-y", "chrome-devtools-mcp"]}}}"#,
        )
        .unwrap();
        match config.servers.get("devtools").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, args, proxy, .. }) => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "chrome-devtools-mcp"]);
                assert!(!proxy);
            }
            other => panic!("expected Stdio server, got {other:?}"),
        }
    }

    #[test]
    fn from_json_accepts_server_proxy_true() {
        let config =
            McpConfig::from_json(r#"{"servers": {"playwright": {"type": "stdio", "command": "npx", "proxy": true}}}"#)
                .unwrap();
        assert!(config.servers.get("playwright").unwrap().proxy());
    }

    #[test]
    fn from_json_rejects_proxy_server_type() {
        let result = McpConfig::from_json(r#"{"servers":{"tools":{"type":"proxy","servers":{}}}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn false_proxy_omits_during_serialization() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "proxy": false}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("proxy"));
    }

    #[test]
    fn true_proxy_serializes() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "proxy": true}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("proxy"));
    }

    #[test]
    fn from_json_rejects_unknown_type() {
        let result = McpConfig::from_json(r#"{"servers": {"bad": {"type": "htp", "url": "https://example.com"}}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn from_json_files_empty_returns_empty_servers() {
        let result = McpConfig::from_json_files::<&str>(&[]).unwrap();
        assert!(result.servers.is_empty());
    }

    #[test]
    fn from_json_files_single_file_matches_from_json_file() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "a.json", &stdio_config("ls"));

        let single = McpConfig::from_json_file(&path).unwrap();
        let multi = McpConfig::from_json_files(&[&path]).unwrap();

        assert_eq!(single.servers.len(), multi.servers.len());
        assert!(multi.servers.contains_key("coding"));
    }

    #[test]
    fn from_json_files_merges_disjoint_servers() {
        let dir = tempdir().unwrap();
        let a = write_config(dir.path(), "a.json", r#"{"servers": {"alpha": {"type": "stdio", "command": "a"}}}"#);
        let b = write_config(dir.path(), "b.json", r#"{"servers": {"beta": {"type": "stdio", "command": "b"}}}"#);

        let merged = McpConfig::from_json_files(&[a, b]).unwrap();
        assert_eq!(merged.servers.len(), 2);
        assert!(merged.servers.contains_key("alpha"));
        assert!(merged.servers.contains_key("beta"));
    }

    #[test]
    fn from_json_files_last_file_wins_on_collision_including_proxy() {
        let dir = tempdir().unwrap();
        let a = write_config(
            dir.path(),
            "a.json",
            r#"{"servers":{"coding":{"type":"stdio","command":"from_a","proxy":true}}}"#,
        );
        let b = write_config(dir.path(), "b.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_b"}}}"#);

        let merged_ab = McpConfig::from_json_files(&[&a, &b]).unwrap();
        match merged_ab.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, proxy, .. }) => {
                assert_eq!(command, "from_b");
                assert!(!proxy);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }

        let merged_ba = McpConfig::from_json_files(&[&b, &a]).unwrap();
        match merged_ba.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, proxy, .. }) => {
                assert_eq!(command, "from_a");
                assert!(*proxy);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn mark_all_proxy_sets_every_server() {
        let mut config = McpConfig::from_json(
            r#"{"servers":{"a":{"type":"stdio","command":"a"},"b":{"type":"http","url":"https://example.com"}}}"#,
        )
        .unwrap();
        config.mark_all_proxy();
        assert!(config.servers.values().all(McpServerConfig::proxy));
    }

    #[test]
    fn from_json_files_propagates_io_error_on_missing_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        let result = McpConfig::from_json_files(&[missing]);
        assert!(matches!(result, Err(ParseError::IoError(_))));
    }

    #[test]
    fn from_json_files_propagates_json_error_on_invalid_file() {
        let dir = tempdir().unwrap();
        let bad = write_config(dir.path(), "bad.json", "not valid json");
        let result = McpConfig::from_json_files(&[bad]);
        assert!(matches!(result, Err(ParseError::JsonError(_))));
    }

    #[tokio::test]
    async fn into_servers_preserves_proxy_flags() {
        let json = r#"{
            "servers": {
                "github": {"type": "stdio", "command": "g"},
                "playwright": {"type": "stdio", "command": "p", "proxy": true}
            }
        }"#;
        let config = McpConfig::from_json(json).unwrap();
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.unwrap();

        assert_eq!(servers.len(), 2);
        assert!(!servers.iter().find(|s| s.name == "github").unwrap().proxy);
        assert!(servers.iter().find(|s| s.name == "playwright").unwrap().proxy);
    }

    #[tokio::test]
    async fn into_servers_with_proxy_forces_proxy_flags() {
        let config =
            McpConfig::from_json(r#"{"servers":{"github":{"type":"stdio","command":"g","proxy":false}}}"#).unwrap();
        let servers = config.into_servers_with_proxy(&HashMap::new(), &Vars::new(), true).await.unwrap();
        assert!(servers[0].proxy);
    }

    #[tokio::test]
    async fn into_transport_expands_workspace_var_in_stdio_args() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"type":"stdio","command":"server","args":["--root","${WORKSPACE}/src"]}}}"#,
        )
        .unwrap();
        let vars = Vars::new().with("WORKSPACE", "/workspace");
        let servers = config.into_servers(&HashMap::new(), &vars).await.unwrap();

        match &servers[0].transport {
            McpTransport::Stdio { args, .. } => {
                assert_eq!(args, &["--root", "/workspace/src"]);
            }
            other => panic!("expected Stdio transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn into_transport_strips_bearer_prefix_from_auth_header() -> Result<(), String> {
        let config = McpConfig::from_json(
            r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer secret-token"}}}}"#,
        )
        .map_err(|e| e.to_string())?;

        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.map_err(|e| e.to_string())?;
        let McpTransport::Http(config) = &servers[0].transport else {
            return Err(format!("expected Http transport, got {:?}", servers[0].transport));
        };

        assert_eq!(config.transport.auth_header.as_deref(), Some("secret-token"));
        Ok(())
    }

    #[tokio::test]
    async fn into_transport_keeps_non_bearer_auth_header_verbatim() -> Result<(), String> {
        let config = McpConfig::from_json(
            r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Basic dXNlcjpwYXNz"}}}}"#,
        )
        .map_err(|e| e.to_string())?;
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.map_err(|e| e.to_string())?;

        let McpTransport::Http(config) = &servers[0].transport else {
            return Err(format!("expected Http transport, got {:?}", servers[0].transport));
        };
        assert_eq!(config.transport.auth_header.as_deref(), Some("Basic dXNlcjpwYXNz"));
        Ok(())
    }

    #[tokio::test]
    async fn into_transport_expands_vars_in_auth_header() -> Result<(), String> {
        let config = McpConfig::from_json(
            r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer ${TOKEN}"}}}}"#,
        )
        .map_err(|e| e.to_string())?;
        let vars = Vars::new().with("TOKEN", "expanded-token");
        let servers = config.into_servers(&HashMap::new(), &vars).await.map_err(|e| e.to_string())?;

        let McpTransport::Http(config) = &servers[0].transport else {
            return Err(format!("expected Http transport, got {:?}", servers[0].transport));
        };
        assert_eq!(config.transport.auth_header.as_deref(), Some("expanded-token"));
        Ok(())
    }
}
