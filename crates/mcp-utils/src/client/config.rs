use aether_auth::OAuthClientRegistration;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
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

    /// Controls which tools are exposed through Aether's shared tool proxy.
    #[serde(default, skip_serializing_if = "ToolExposure::is_direct")]
    pub proxy: ToolExposure,
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

    /// Controls which tools are exposed through Aether's shared tool proxy.
    #[serde(default, skip_serializing_if = "ToolExposure::is_direct")]
    pub proxy: ToolExposure,
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

    /// Controls which tools are exposed through Aether's shared tool proxy.
    #[serde(default, skip_serializing_if = "ToolExposure::is_direct")]
    pub proxy: ToolExposure,
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

/// Which of a server's tools are routed through Aether's shared tool proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(from = "ToolExposureConfig", into = "ToolExposureConfig")]
#[schemars(with = "ToolExposureConfig")]
pub enum ToolExposure {
    #[default]
    Direct,
    Proxied(ToolProxyRules),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolProxyRules {
    /// Tool names to proxy. An empty list includes every tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,

    /// Tool names to avoid proxying and directly expose. Exclude rules take precedence over include rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    pub tool_exposure: ToolExposure,
}

#[derive(Debug, Clone)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Http(McpHttpConfig),
    InMemory { spec: InMemoryServerSpec },
}

#[derive(Clone, Debug)]
pub struct InMemoryServerSpec {
    pub factory: String,
    pub args: Vec<String>,
    pub input: Option<Value>,
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
    pub fn proxied_all() -> Self {
        Self::Proxied(ToolProxyRules::default())
    }

    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct)
    }

    pub fn is_proxied(&self) -> bool {
        matches!(self, Self::Proxied(_))
    }

    pub fn is_direct_tool(&self, tool_name: &str) -> bool {
        match self {
            Self::Direct => true,
            Self::Proxied(rules) => !rules.matches(tool_name),
        }
    }

    /// Route every tool through the proxy, preserving any existing rules.
    pub fn mark_proxied(&mut self) {
        if self.is_direct() {
            *self = Self::proxied_all();
        }
    }
}

impl ToolProxyRules {
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

/// The `proxy` config field's wire shape: a boolean or an include/exclude object.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum ToolExposureConfig {
    Enabled(bool),
    Rules(ToolProxyRules),
}

impl From<ToolExposureConfig> for ToolExposure {
    fn from(repr: ToolExposureConfig) -> Self {
        match repr {
            ToolExposureConfig::Enabled(false) => Self::Direct,
            ToolExposureConfig::Enabled(true) => Self::proxied_all(),
            ToolExposureConfig::Rules(rules) => Self::Proxied(rules),
        }
    }
}

impl From<ToolExposure> for ToolExposureConfig {
    fn from(exposure: ToolExposure) -> Self {
        match exposure {
            ToolExposure::Direct => Self::Enabled(false),
            ToolExposure::Proxied(rules) if rules == ToolProxyRules::default() => Self::Enabled(true),
            ToolExposure::Proxied(rules) => Self::Rules(rules),
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

    pub fn is_proxied(&self) -> bool {
        self.tool_exposure.is_proxied()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Variable expansion failed: {0}")]
    VarError(#[from] VarError),

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

    pub fn into_servers(self, vars: &Vars) -> Result<Vec<McpServer>, ParseError> {
        self.into_servers_with_proxy(vars, false)
    }

    pub fn into_servers_with_proxy(self, vars: &Vars, force_proxy: bool) -> Result<Vec<McpServer>, ParseError> {
        self.servers.into_iter().map(|(name, config)| config.into_server(name, vars, force_proxy)).collect()
    }

    pub fn mark_all_proxy(&mut self) {
        for server in self.servers.values_mut() {
            server.mark_proxied();
        }
    }
}

impl McpServerConfig {
    pub fn proxy(&self) -> &ToolExposure {
        match self {
            McpServerConfig::Stdio(config) => &config.proxy,
            McpServerConfig::Remote(config) => &config.proxy,
            McpServerConfig::InMemory(config) => &config.proxy,
        }
    }

    pub fn mark_proxied(&mut self) {
        let proxy = match self {
            McpServerConfig::Stdio(config) => &mut config.proxy,
            McpServerConfig::Remote(config) => &mut config.proxy,
            McpServerConfig::InMemory(config) => &mut config.proxy,
        };
        proxy.mark_proxied();
    }

    pub fn into_server(self, name: String, vars: &Vars, force_proxy: bool) -> Result<McpServer, ParseError> {
        let mut exposure = self.proxy().clone();
        if force_proxy {
            exposure.mark_proxied();
        }
        let transport = self.into_transport(name.clone(), vars)?;
        Ok(McpServer { name, transport, tool_exposure: exposure })
    }

    fn into_transport(self, name: String, vars: &Vars) -> Result<McpTransport, ParseError> {
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
                let args = args.into_iter().map(|a| vars.expand(&a)).collect::<Result<Vec<_>, VarError>>()?;
                Ok(McpTransport::InMemory { spec: InMemoryServerSpec { factory: name, args, input } })
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
                assert!(proxy.is_direct());
            }
            other => panic!("expected Stdio server, got {other:?}"),
        }
    }

    #[test]
    fn from_json_accepts_server_proxy_true() {
        let config =
            McpConfig::from_json(r#"{"servers": {"playwright": {"type": "stdio", "command": "npx", "proxy": true}}}"#)
                .unwrap();
        assert!(config.servers.get("playwright").unwrap().proxy().is_proxied());
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
            r#"{"servers":{"coding":{"type":"stdio","command":"from_a","proxy":{"exclude":["bash"]}}}}"#,
        );
        let b = write_config(dir.path(), "b.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_b"}}}"#);

        let merged_ab = McpConfig::from_json_files(&[&a, &b]).unwrap();
        match merged_ab.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, proxy, .. }) => {
                assert_eq!(command, "from_b");
                assert_eq!(proxy, &ToolExposure::Direct);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }

        let merged_ba = McpConfig::from_json_files(&[&b, &a]).unwrap();
        match merged_ba.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, proxy, .. }) => {
                assert_eq!(command, "from_a");
                assert_eq!(proxy, &ToolExposure::Proxied(ToolProxyRules::new(&[], &["bash"])));
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
        assert!(config.servers.values().all(|server| server.proxy().is_proxied()));
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
        let servers = config.into_servers(&Vars::new()).unwrap();

        assert_eq!(servers.len(), 2);
        assert!(!servers.iter().find(|s| s.name == "github").unwrap().is_proxied());
        assert!(servers.iter().find(|s| s.name == "playwright").unwrap().is_proxied());
    }

    #[tokio::test]
    async fn into_servers_with_proxy_forces_proxy_flags() {
        let config =
            McpConfig::from_json(r#"{"servers":{"github":{"type":"stdio","command":"g","proxy":false}}}"#).unwrap();
        let servers = config.into_servers_with_proxy(&Vars::new(), true).unwrap();
        assert!(servers[0].is_proxied());
    }

    #[test]
    fn proxy_accepts_boolean_or_include_exclude_rules_for_all_transport_shapes() {
        let config = McpConfig::from_json(
            r#"{"servers":{"all":{"command":"a","proxy":true},"stdio":{"command":"x","proxy":{"include":["lsp_*"],"exclude":["lsp_rename"]}},"http":{"type":"http","url":"https://example.com","proxy":{"exclude":["bash"]}},"memory":{"type":"in-memory","proxy":{"include":["read"]}}}}"#,
        )
        .unwrap();

        assert_eq!(config.servers["all"].proxy(), &ToolExposure::proxied_all());
        assert_eq!(
            config.servers["stdio"].proxy(),
            &ToolExposure::Proxied(ToolProxyRules::new(&["lsp_*"], &["lsp_rename"]))
        );
        assert_eq!(config.servers["http"].proxy(), &ToolExposure::Proxied(ToolProxyRules::new(&[], &["bash"])));
        assert_eq!(config.servers["memory"].proxy(), &ToolExposure::Proxied(ToolProxyRules::new(&["read"], &[])));
    }

    #[test]
    fn proxy_rules_serialize_inside_proxy_and_defaults_are_omitted() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"command":"x","proxy":{"exclude":["bash","lsp_*"]}},"direct":{"command":"y"},"full":{"command":"z","proxy":true}}}"#,
        )
        .unwrap();
        let value = serde_json::to_value(config).unwrap();

        assert_eq!(value["servers"]["coding"]["proxy"], serde_json::json!({"exclude":["bash", "lsp_*"]}));
        assert!(value["servers"]["direct"].get("proxy").is_none());
        assert_eq!(value["servers"]["full"]["proxy"], serde_json::json!(true));
    }

    #[test]
    fn legacy_direct_tools_is_rejected() {
        let result =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"x","proxy":true,"direct_tools":["bash"]}}}"#);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn proxy_rules_partition_tools_with_exclude_winning() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"command":"server","proxy":{"include":["lsp_*","bash"],"exclude":["lsp_rename"]}}}}"#,
        )
        .unwrap();
        let servers = config.into_servers(&Vars::new()).unwrap();
        let exposure = &servers[0].tool_exposure;

        assert!(!exposure.is_direct_tool("lsp_hover"));
        assert!(exposure.is_direct_tool("lsp_rename"));
        assert!(!exposure.is_direct_tool("bash"));
        assert!(exposure.is_direct_tool("read_file"));
    }

    #[tokio::test]
    async fn forced_proxy_preserves_per_server_proxy_rules() {
        let config =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"server","proxy":{"exclude":["bash"]}}}}"#)
                .unwrap();
        let servers = config.into_servers_with_proxy(&Vars::new(), true).unwrap();
        assert!(servers[0].is_proxied());
        assert!(servers[0].tool_exposure.is_direct_tool("bash"));
        assert!(!servers[0].tool_exposure.is_direct_tool("read_file"));
    }

    #[tokio::test]
    async fn into_transport_expands_workspace_var_in_stdio_args() {
        let config = McpConfig::from_json(
            r#"{"servers":{"coding":{"type":"stdio","command":"server","args":["--root","${WORKSPACE}/src"]}}}"#,
        )
        .unwrap();
        let vars = Vars::new().with("WORKSPACE", "/workspace");
        let servers = config.into_servers(&vars).unwrap();

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

        let servers = config.into_servers(&Vars::new()).map_err(|e| e.to_string())?;
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
        let servers = config.into_servers(&Vars::new()).map_err(|e| e.to_string())?;

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
        let servers = config.into_servers(&vars).map_err(|e| e.to_string())?;

        let McpTransport::Http(config) = &servers[0].transport else {
            return Err(format!("expected Http transport, got {:?}", servers[0].transport));
        };
        assert_eq!(config.transport.auth_header.as_deref(), Some("expanded-token"));
        Ok(())
    }
}
