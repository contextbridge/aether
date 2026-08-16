use aether_auth::OAuthClientRegistration;
use futures::future::BoxFuture;
use rmcp::{RoleServer, service::DynService, transport::streamable_http_client::StreamableHttpClientTransportConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::num::NonZeroU16;
use std::path::Path;
use utils::matches_name_pattern;
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

    /// Controls which tools are available only through progressive discovery.
    #[serde(rename = "deferTools", default, skip_serializing_if = "ToolExposure::is_fully_model_visible")]
    pub defer_tools: ToolExposure,
}

pub const AETHER_OAUTH_CLIENT_METADATA_URL: &str = "https://aether-agent.io/oauth/client-metadata.json";
pub const AETHER_OAUTH_CALLBACK_PORT: NonZeroU16 = NonZeroU16::new(3118).unwrap();

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpOAuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<NonZeroU16>,
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

    /// Controls which tools are available only through progressive discovery.
    #[serde(rename = "deferTools", default, skip_serializing_if = "ToolExposure::is_fully_model_visible")]
    pub defer_tools: ToolExposure,
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

    /// Controls which tools are available only through progressive discovery.
    #[serde(rename = "deferTools", default, skip_serializing_if = "ToolExposure::is_fully_model_visible")]
    pub defer_tools: ToolExposure,
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

/// Which of a server's tools are available only through progressive discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(from = "ToolExposureConfig", into = "ToolExposureConfig")]
#[schemars(with = "ToolExposureConfig")]
pub enum ToolExposure {
    #[default]
    ModelVisible,
    Deferred(DeferredToolRules),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeferredToolRules {
    /// Tool names to select for progressive discovery. An empty list includes every tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,

    /// Tool names to keep model-visible. Exclude rules take precedence over include rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    pub tool_exposure: ToolExposure,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOAuth {
    pub client_registration: OAuthClientRegistration,
    pub callback_port: NonZeroU16,
}

impl ResolvedOAuth {
    pub fn redirect_uri(&self) -> String {
        loopback_redirect_uri(self.callback_port.get())
    }
}

pub fn loopback_redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/")
}

impl McpHttpConfig {
    pub fn resolved_oauth(&self) -> Option<ResolvedOAuth> {
        if self.transport.auth_header.is_some() {
            return None;
        }
        let oauth = self.oauth.as_ref();
        let client_registration = match oauth {
            Some(McpOAuthConfig { client_id: Some(client_id), .. }) => {
                OAuthClientRegistration::PreRegistered(client_id.clone())
            }
            Some(McpOAuthConfig { client_metadata_url: Some(url), .. }) => {
                OAuthClientRegistration::ClientMetadata(url.clone())
            }
            _ => OAuthClientRegistration::ClientMetadata(AETHER_OAUTH_CLIENT_METADATA_URL.to_string()),
        };
        Some(ResolvedOAuth {
            client_registration,
            callback_port: oauth.and_then(|oauth| oauth.callback_port).unwrap_or(AETHER_OAUTH_CALLBACK_PORT),
        })
    }
}

impl From<StreamableHttpClientTransportConfig> for McpHttpConfig {
    fn from(transport: StreamableHttpClientTransportConfig) -> Self {
        Self { transport, oauth: None }
    }
}

impl ToolExposure {
    pub fn deferred_all() -> Self {
        Self::Deferred(DeferredToolRules::default())
    }

    pub fn is_fully_model_visible(&self) -> bool {
        matches!(self, Self::ModelVisible)
    }

    pub fn has_deferred_tools(&self) -> bool {
        matches!(self, Self::Deferred(_))
    }

    pub fn is_model_visible(&self, tool_name: &str) -> bool {
        !self.is_deferred(tool_name)
    }

    pub fn is_deferred(&self, tool_name: &str) -> bool {
        match self {
            Self::ModelVisible => false,
            Self::Deferred(rules) => rules.matches(tool_name),
        }
    }

    /// Route every tool through progressive discovery, preserving any existing rules.
    pub fn defer_all(&mut self) {
        if self.is_fully_model_visible() {
            *self = Self::deferred_all();
        }
    }
}

impl DeferredToolRules {
    pub fn new(include: &[&str], exclude: &[&str]) -> Self {
        Self {
            include: include.iter().map(ToString::to_string).collect(),
            exclude: exclude.iter().map(ToString::to_string).collect(),
        }
    }

    fn matches(&self, tool_name: &str) -> bool {
        let included =
            self.include.is_empty() || self.include.iter().any(|pattern| matches_name_pattern(pattern, tool_name));
        let excluded = self.exclude.iter().any(|pattern| matches_name_pattern(pattern, tool_name));
        included && !excluded
    }
}

/// The `deferTools` config field's wire shape: a boolean or an include/exclude object.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum ToolExposureConfig {
    Enabled(bool),
    Rules(DeferredToolRules),
}

impl From<ToolExposureConfig> for ToolExposure {
    fn from(repr: ToolExposureConfig) -> Self {
        match repr {
            ToolExposureConfig::Enabled(false) => Self::ModelVisible,
            ToolExposureConfig::Enabled(true) => Self::deferred_all(),
            ToolExposureConfig::Rules(rules) => Self::Deferred(rules),
        }
    }
}

impl From<ToolExposure> for ToolExposureConfig {
    fn from(exposure: ToolExposure) -> Self {
        match exposure {
            ToolExposure::ModelVisible => Self::Enabled(false),
            ToolExposure::Deferred(rules) if rules == DeferredToolRules::default() => Self::Enabled(true),
            ToolExposure::Deferred(rules) => Self::Rules(rules),
        }
    }
}

impl McpServer {
    pub fn new(name: impl Into<String>, transport: McpTransport, tool_exposure: ToolExposure) -> Self {
        Self { name: name.into(), transport, tool_exposure }
    }

    pub fn with_exposure(mut self, exposure: ToolExposure) -> Self {
        self.tool_exposure = exposure;
        self
    }

    pub fn has_deferred_tools(&self) -> bool {
        self.tool_exposure.has_deferred_tools()
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
        Ok(Self { name: self.name.clone(), transport, tool_exposure: self.tool_exposure.clone() })
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
            .field("tool_exposure", &self.tool_exposure)
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

    #[error("Invalid nested config in progressive discovery: {0}")]
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
        let mut servers = Vec::with_capacity(self.servers.len());
        for (name, config) in self.servers {
            servers.push(config.into_server(name, factories, vars).await?);
        }
        Ok(servers)
    }

    pub fn defer_all_tools(&mut self) {
        for server in self.servers.values_mut() {
            server.defer_all();
        }
    }
}

impl McpServerConfig {
    pub fn tool_exposure(&self) -> &ToolExposure {
        match self {
            McpServerConfig::Stdio(config) => &config.defer_tools,
            McpServerConfig::Remote(config) => &config.defer_tools,
            McpServerConfig::InMemory(config) => &config.defer_tools,
        }
    }

    pub fn defer_all(&mut self) {
        let exposure = match self {
            McpServerConfig::Stdio(config) => &mut config.defer_tools,
            McpServerConfig::Remote(config) => &mut config.defer_tools,
            McpServerConfig::InMemory(config) => &mut config.defer_tools,
        };
        exposure.defer_all();
    }

    pub async fn into_server(
        self,
        name: String,
        factories: &HashMap<String, ServerFactory>,
        vars: &Vars,
    ) -> Result<McpServer, ParseError> {
        let exposure = self.tool_exposure().clone();
        let transport = self.into_transport(name.clone(), factories, vars).await?;
        Ok(McpServer { name, transport, tool_exposure: exposure })
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
                        Ok(McpOAuthConfig {
                            client_id: oauth.client_id.map(|value| vars.expand(&value)).transpose()?,
                            client_metadata_url: oauth
                                .client_metadata_url
                                .map(|value| vars.expand(&value))
                                .transpose()?,
                            callback_port: oauth.callback_port,
                        })
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
            McpServerConfig::Stdio(StdioServerConfig { command, args, defer_tools: deferred, .. }) => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "chrome-devtools-mcp"]);
                assert!(deferred.is_fully_model_visible());
            }
            other => panic!("expected Stdio server, got {other:?}"),
        }
    }

    #[test]
    fn from_json_accepts_server_deferred_true() {
        let config = McpConfig::from_json(
            r#"{"servers": {"playwright": {"type": "stdio", "command": "npx", "deferTools": true}}}"#,
        )
        .unwrap();
        assert!(config.servers.get("playwright").unwrap().tool_exposure().has_deferred_tools());
    }

    #[test]
    fn from_json_rejects_deferred_server_type() {
        let result = McpConfig::from_json(r#"{"servers":{"tools":{"type":"proxy","servers":{}}}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn false_deferred_omits_during_serialization() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "deferTools": false}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("deferTools"));
    }

    #[test]
    fn true_deferred_serializes() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "deferTools": true}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains(r#""deferTools":true"#));
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
    fn from_json_rejects_unknown_exposure_fields_for_all_transports() {
        for server in [
            r#"{"command":"x","direct_tool":["bash"]}"#,
            r#"{"type":"http","url":"https://example.com","direct_tool":["bash"]}"#,
            r#"{"type":"in-memory","direct_tool":["bash"]}"#,
        ] {
            let json = format!(r#"{{"servers":{{"bad":{server}}}}}"#);
            assert!(McpConfig::from_json(&json).is_err(), "unknown field was accepted: {server}");
        }
    }

    #[test]
    fn from_json_files_last_file_wins_on_collision_including_exposure() {
        let dir = tempdir().unwrap();
        let a = write_config(
            dir.path(),
            "a.json",
            r#"{"servers":{"coding":{"type":"stdio","command":"from_a","deferTools":{"exclude":["bash"]}}}}"#,
        );
        let b = write_config(dir.path(), "b.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_b"}}}"#);

        let merged_ab = McpConfig::from_json_files(&[&a, &b]).unwrap();
        match merged_ab.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, defer_tools: deferred, .. }) => {
                assert_eq!(command, "from_b");
                assert_eq!(deferred, &ToolExposure::ModelVisible);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }

        let merged_ba = McpConfig::from_json_files(&[&b, &a]).unwrap();
        match merged_ba.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, defer_tools: deferred, .. }) => {
                assert_eq!(command, "from_a");
                assert_eq!(deferred, &ToolExposure::Deferred(DeferredToolRules::new(&[], &["bash"])));
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn mark_all_deferred_sets_every_server() {
        let mut config = McpConfig::from_json(
            r#"{"servers":{"a":{"type":"stdio","command":"a"},"b":{"type":"http","url":"https://example.com"}}}"#,
        )
        .unwrap();
        config.defer_all_tools();
        assert!(config.servers.values().all(|server| server.tool_exposure().has_deferred_tools()));
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
    async fn into_servers_preserves_deferred_flags() {
        let json = r#"{
            "servers": {
                "github": {"type": "stdio", "command": "g"},
                "playwright": {"type": "stdio", "command": "p", "deferTools": true}
            }
        }"#;
        let config = McpConfig::from_json(json).unwrap();
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.unwrap();

        assert_eq!(servers.len(), 2);
        assert!(!servers.iter().find(|s| s.name == "github").unwrap().has_deferred_tools());
        assert!(servers.iter().find(|s| s.name == "playwright").unwrap().has_deferred_tools());
    }

    #[tokio::test]
    async fn into_servers_with_deferred_forces_deferred_flags() {
        let mut config =
            McpConfig::from_json(r#"{"servers":{"github":{"type":"stdio","command":"g","deferTools":false}}}"#)
                .unwrap();
        config.defer_all_tools();
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.unwrap();
        assert!(servers[0].has_deferred_tools());
    }

    #[test]
    fn deferred_accepts_boolean_or_include_exclude_rules_for_all_transport_shapes() {
        let config = McpConfig::from_json(
            r#"{"servers":{"all":{"command":"a","deferTools":true},"stdio":{"command":"x","deferTools":{"include":["lsp_*"],"exclude":["lsp_rename"]}},"http":{"type":"http","url":"https://example.com","deferTools":{"exclude":["bash"]}},"memory":{"type":"in-memory","deferTools":{"include":["read"]}}}}"#,
        )
        .unwrap();

        assert_eq!(config.servers["all"].tool_exposure(), &ToolExposure::deferred_all());
        assert_eq!(
            config.servers["stdio"].tool_exposure(),
            &ToolExposure::Deferred(DeferredToolRules::new(&["lsp_*"], &["lsp_rename"]))
        );
        assert_eq!(
            config.servers["http"].tool_exposure(),
            &ToolExposure::Deferred(DeferredToolRules::new(&[], &["bash"]))
        );
        assert_eq!(
            config.servers["memory"].tool_exposure(),
            &ToolExposure::Deferred(DeferredToolRules::new(&["read"], &[]))
        );
    }

    #[test]
    fn deferred_rules_serialize_inside_deferred_and_defaults_are_omitted() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"command":"x","deferTools":{"exclude":["bash","lsp_*"]}},"direct":{"command":"y"},"full":{"command":"z","deferTools":true}}}"#,
        )
        .unwrap();
        let value = serde_json::to_value(config).unwrap();

        assert_eq!(value["servers"]["coding"]["deferTools"], serde_json::json!({"exclude":["bash", "lsp_*"]}));
        assert!(value["servers"]["direct"].get("deferTools").is_none());
        assert_eq!(value["servers"]["full"]["deferTools"], serde_json::json!(true));
    }

    #[test]
    fn legacy_direct_tools_is_rejected() {
        let result =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"x","deferTools":true,"direct_tools":["bash"]}}}"#);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deferred_rules_partition_tools_with_exclude_winning() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"command":"server","deferTools":{"include":["lsp_*","bash"],"exclude":["lsp_rename"]}}}}"#,
        )
        .unwrap();
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.unwrap();
        let exposure = &servers[0].tool_exposure;

        assert!(!exposure.is_model_visible("lsp_hover"));
        assert!(exposure.is_model_visible("lsp_rename"));
        assert!(!exposure.is_model_visible("bash"));
        assert!(exposure.is_model_visible("read_file"));
    }

    #[tokio::test]
    async fn forced_deferred_preserves_per_server_deferred_rules() {
        let mut config =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"server","deferTools":{"exclude":["bash"]}}}}"#)
                .unwrap();
        config.defer_all_tools();
        let servers = config.into_servers(&HashMap::new(), &Vars::new()).await.unwrap();
        assert!(servers[0].has_deferred_tools());
        assert!(servers[0].tool_exposure.is_model_visible("bash"));
        assert!(!servers[0].tool_exposure.is_model_visible("read_file"));
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
