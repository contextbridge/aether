use crate::client::Transport;
use crate::gateway::ToolExposure;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, InvalidHeaderName, InvalidHeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU16;
use std::path::Path;
use utils::variables::{VarError, Vars};

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct McpConfig {
    #[serde(alias = "mcpServers")]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[doc = include_str!("docs/mcp_server_config.md")]
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
    #[serde(rename = "type", default)]
    pub type_: StdioType,

    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(rename = "deferTools", alias = "proxy", default, skip_serializing_if = "ToolExposure::is_model_visible")]
    pub defer_tools: ToolExposure,
}

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
    #[serde(rename = "type")]
    pub type_: RemoteType,

    pub url: String,

    #[serde(default)]
    pub headers: HashMap<String, String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,

    #[serde(rename = "deferTools", alias = "proxy", default, skip_serializing_if = "ToolExposure::is_model_visible")]
    pub defer_tools: ToolExposure,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InMemoryServerConfig {
    #[serde(rename = "type")]
    pub type_: InMemoryType,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(rename = "deferTools", alias = "proxy", default, skip_serializing_if = "ToolExposure::is_model_visible")]
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

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Variable expansion failed: {0}")]
    VarError(#[from] VarError),

    #[error("Invalid HTTP header name {name:?}: {source}")]
    InvalidHeaderName {
        name: String,
        #[source]
        source: InvalidHeaderName,
    },

    #[error("Invalid HTTP header value for {name:?}: {source}")]
    InvalidHeaderValue {
        name: String,
        #[source]
        source: InvalidHeaderValue,
    },
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

    pub fn defer_all_tools(&mut self) {
        for server in self.servers.values_mut() {
            server.defer_tools_mut().defer_all_tools();
        }
    }
}

impl McpServerConfig {
    pub fn defer_tools(&self) -> &ToolExposure {
        match self {
            McpServerConfig::Stdio(config) => &config.defer_tools,
            McpServerConfig::Remote(config) => &config.defer_tools,
            McpServerConfig::InMemory(config) => &config.defer_tools,
        }
    }

    pub fn defer_tools_mut(&mut self) -> &mut ToolExposure {
        match self {
            McpServerConfig::Stdio(config) => &mut config.defer_tools,
            McpServerConfig::Remote(config) => &mut config.defer_tools,
            McpServerConfig::InMemory(config) => &mut config.defer_tools,
        }
    }
}

impl StdioServerConfig {
    pub fn into_transport(self, vars: &Vars) -> Result<Transport, ParseError> {
        let env = self.env.into_iter().map(|(name, value)| Ok((name, vars.expand(&value)?)));
        Ok(Transport::Stdio {
            command: vars.expand(&self.command)?,
            args: expand_all(vars, &self.args)?,
            env: env.collect::<Result<_, VarError>>()?,
        })
    }
}

impl RemoteServerConfig {
    /// A Streamable HTTP server at `url`, with no headers.
    pub fn http(url: impl Into<String>) -> Self {
        Self {
            type_: RemoteType::Http,
            url: url.into(),
            headers: HashMap::new(),
            oauth: None,
            defer_tools: ToolExposure::default(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    /// Names of the `${VAR}` references [`Self::into_transport`] expands in the URL, headers and OAuth settings.
    pub fn references(&self) -> BTreeSet<String> {
        let vars = Vars::new();
        let oauth = self.oauth.iter().flat_map(|oauth| [&oauth.client_id, &oauth.client_metadata_url]).flatten();
        std::iter::once(&self.url)
            .chain(self.headers.values())
            .chain(oauth)
            .flat_map(|template| vars.references(template))
            .collect()
    }

    /// Expands `${VAR}` references with `vars`. Every header value is marked sensitive, since headers usually
    /// carry credentials.
    pub fn into_transport(self, vars: &Vars) -> Result<Transport, ParseError> {
        let mut headers = HeaderMap::with_capacity(self.headers.len());
        for (name, value) in self.headers {
            let header_name = name
                .parse::<HeaderName>()
                .map_err(|source| ParseError::InvalidHeaderName { name: name.clone(), source })?;
            let mut header_value = vars
                .expand(&value)?
                .parse::<HeaderValue>()
                .map_err(|source| ParseError::InvalidHeaderValue { name, source })?;
            header_value.set_sensitive(true);
            headers.insert(header_name, header_value);
        }
        let oauth = self.oauth.map(|oauth| expand_oauth(vars, oauth)).transpose()?;
        Ok(Transport::Http { url: vars.expand(&self.url)?, headers, oauth })
    }
}

impl InMemoryServerConfig {
    pub fn expand(mut self, vars: &Vars) -> Result<Self, ParseError> {
        self.args = expand_all(vars, &self.args)?;
        Ok(self)
    }
}

fn expand_all(vars: &Vars, values: &[String]) -> Result<Vec<String>, VarError> {
    values.iter().map(|value| vars.expand(value)).collect()
}

fn expand_oauth(vars: &Vars, oauth: McpOAuthConfig) -> Result<McpOAuthConfig, VarError> {
    let expand = |value: Option<String>| value.map(|value| vars.expand(&value)).transpose();
    Ok(McpOAuthConfig {
        client_id: expand(oauth.client_id)?,
        client_metadata_url: expand(oauth.client_metadata_url)?,
        callback_port: oauth.callback_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{ToolFilter, ToolMatcher};
    use reqwest::header::AUTHORIZATION;
    use rmcp::model::{Tool, ToolAnnotations};
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn write_config(dir: &Path, name: &str, json: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, json).unwrap();
        path
    }

    fn stdio_config(command: &str) -> String {
        format!(r#"{{"servers": {{"coding": {{"type": "stdio", "command": "{command}"}}}}}}"#)
    }

    fn only_server(json: &str) -> McpServerConfig {
        McpConfig::from_json(json).unwrap().servers.into_values().next().expect("one server")
    }

    fn deferred(allow: &[&str], deny: &[&str]) -> ToolExposure {
        let matchers = |patterns: &[&str]| patterns.iter().copied().map(ToolMatcher::name).collect();
        ToolExposure::Deferred(ToolFilter { allow: matchers(allow), deny: matchers(deny) })
    }

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
    }

    fn transport(json: &str, vars: &Vars) -> Result<Transport, ParseError> {
        match only_server(json) {
            McpServerConfig::Stdio(config) => config.into_transport(vars),
            McpServerConfig::Remote(config) => config.into_transport(vars),
            McpServerConfig::InMemory(_) => panic!("in-memory servers have no transport"),
        }
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
            McpServerConfig::Stdio(StdioServerConfig { command, args, defer_tools: exposure, .. }) => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "chrome-devtools-mcp"]);
                assert!(!exposure.has_deferred_tools());
            }
            other => panic!("expected Stdio server, got {other:?}"),
        }
    }

    #[test]
    fn from_json_accepts_legacy_server_proxy_true() {
        let config =
            McpConfig::from_json(r#"{"servers": {"playwright": {"type": "stdio", "command": "npx", "proxy": true}}}"#)
                .unwrap();
        assert!(config.servers.get("playwright").unwrap().defer_tools().has_deferred_tools());
    }

    #[test]
    fn from_json_accepts_server_defer_tools_true() {
        let config = McpConfig::from_json(
            r#"{"servers": {"playwright": {"type": "stdio", "command": "npx", "deferTools": true}}}"#,
        )
        .unwrap();
        assert!(config.servers.get("playwright").unwrap().defer_tools().has_deferred_tools());
    }

    #[test]
    fn from_json_rejects_unknown_server_type() {
        let result = McpConfig::from_json(r#"{"servers":{"tools":{"type":"deferTools","servers":{}}}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn false_defer_tools_omits_during_serialization() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "deferTools": false}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("deferTools"));
    }

    #[test]
    fn true_defer_tools_serializes() {
        let config =
            McpConfig::from_json(r#"{"servers": {"coding": {"type": "stdio", "command": "a", "deferTools": true}}}"#)
                .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("deferTools"));
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
            McpServerConfig::Stdio(StdioServerConfig { command, defer_tools: exposure, .. }) => {
                assert_eq!(command, "from_b");
                assert_eq!(exposure, &ToolExposure::ModelVisible);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }

        let merged_ba = McpConfig::from_json_files(&[&b, &a]).unwrap();
        match merged_ba.servers.get("coding").unwrap() {
            McpServerConfig::Stdio(StdioServerConfig { command, defer_tools: exposure, .. }) => {
                assert_eq!(command, "from_a");
                assert_eq!(exposure, &deferred(&[], &["bash"]));
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn defer_all_tools_sets_every_server() {
        let mut config = McpConfig::from_json(
            r#"{"servers":{"a":{"type":"stdio","command":"a"},"b":{"type":"http","url":"https://example.com"}}}"#,
        )
        .unwrap();
        config.defer_all_tools();
        assert!(config.servers.values().all(|server| server.defer_tools().has_deferred_tools()));
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

    #[test]
    fn defer_tools_accepts_boolean_or_rules_for_all_transport_shapes() {
        let config = McpConfig::from_json(
            r#"{"servers":{"all":{"command":"a","deferTools":true},"stdio":{"command":"x","deferTools":{"include":["lsp_*"],"exclude":["lsp_rename"]}},"http":{"type":"http","url":"https://example.com","deferTools":{"exclude":["bash"]}},"memory":{"type":"in-memory","deferTools":{"include":["read"]}}}}"#,
        )
        .unwrap();

        assert_eq!(config.servers["all"].defer_tools(), &ToolExposure::deferred_all());
        assert_eq!(config.servers["stdio"].defer_tools(), &deferred(&["lsp_*"], &["lsp_rename"]));
        assert_eq!(config.servers["http"].defer_tools(), &deferred(&[], &["bash"]));
        assert_eq!(config.servers["memory"].defer_tools(), &deferred(&["read"], &[]));
    }

    #[test]
    fn deferred_tool_rules_serialize_and_defaults_are_omitted() {
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
    fn defer_tools_rejects_unknown_rule_fields() {
        for rules in [r#"{"allow":["lsp_*"]}"#, r#"{"deny":["bash"]}"#, r#"{"include":["lsp_*"],"direct":["bash"]}"#] {
            let json = format!(r#"{{"servers":{{"coding":{{"command":"x","deferTools":{rules}}}}}}}"#);
            assert!(McpConfig::from_json(&json).is_err(), "unknown deferTools field was accepted: {rules}");
        }
    }

    #[test]
    fn defer_tools_matches_tool_annotations() {
        let server =
            only_server(r#"{"servers":{"coding":{"command":"x","deferTools":{"exclude":[{"readOnly":true}]}}}}"#);
        let mut read_only = tool("read_file");
        read_only.annotations = Some(ToolAnnotations::new().read_only(true));

        assert!(!server.defer_tools().defers(&read_only));
        assert!(server.defer_tools().defers(&tool("write_file")));
    }

    #[test]
    fn legacy_direct_tools_is_rejected() {
        let result =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"x","deferTools":true,"direct_tools":["bash"]}}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn deferred_tool_rules_partition_tools_with_exclude_winning() {
        let server = only_server(
            r#"{"servers":{"coding":{"command":"server","deferTools":{"include":["lsp_*","bash"],"exclude":["lsp_rename"]}}}}"#,
        );
        let exposure = server.defer_tools();

        assert!(exposure.defers(&tool("lsp_hover")));
        assert!(!exposure.defers(&tool("lsp_rename")));
        assert!(exposure.defers(&tool("bash")));
        assert!(!exposure.defers(&tool("read_file")));
    }

    #[test]
    fn forced_deferral_preserves_per_server_rules() {
        let mut config =
            McpConfig::from_json(r#"{"servers":{"coding":{"command":"server","deferTools":{"exclude":["bash"]}}}}"#)
                .unwrap();
        config.defer_all_tools();
        let exposure = config.servers["coding"].defer_tools();

        assert!(exposure.has_deferred_tools());
        assert!(!exposure.defers(&tool("bash")));
        assert!(exposure.defers(&tool("read_file")));
    }

    #[test]
    fn stdio_transport_expands_workspace_var_in_args() {
        let json = r#"{"servers":{"coding":{"type":"stdio","command":"server","args":["--root","${WORKSPACE}/src"]}}}"#;
        let vars = Vars::new().with("WORKSPACE", "/workspace");

        let Transport::Stdio { args, .. } = transport(json, &vars).unwrap() else { panic!("expected stdio") };
        assert_eq!(args, ["--root", "/workspace/src"]);
    }

    #[test]
    fn http_transport_marks_bearer_auth_header_sensitive() {
        let json = r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer secret-token"}}}}"#;

        let Transport::Http { headers, .. } = transport(json, &Vars::new()).unwrap() else { panic!("expected http") };
        assert_eq!(headers[AUTHORIZATION], "Bearer secret-token");
        assert!(headers[AUTHORIZATION].is_sensitive());
    }

    #[test]
    fn http_transport_keeps_non_bearer_auth_header_verbatim() {
        let json = r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Basic dXNlcjpwYXNz"}}}}"#;

        let Transport::Http { headers, .. } = transport(json, &Vars::new()).unwrap() else { panic!("expected http") };
        assert_eq!(headers[AUTHORIZATION], "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn http_transport_expands_vars_in_auth_header() {
        let json = r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer ${TOKEN}"}}}}"#;
        let vars = Vars::new().with("TOKEN", "expanded-token");

        let Transport::Http { headers, .. } = transport(json, &vars).unwrap() else { panic!("expected http") };
        assert_eq!(headers[AUTHORIZATION], "Bearer expanded-token");
    }

    #[test]
    fn http_transport_rejects_invalid_header_value() {
        let json = r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"X-Key":"bad\nvalue"}}}}"#;

        let result = transport(json, &Vars::new());
        assert!(matches!(result, Err(ParseError::InvalidHeaderValue { name, .. }) if name == "X-Key"));
    }

    #[test]
    fn in_memory_config_expands_args() {
        let McpServerConfig::InMemory(config) =
            only_server(r#"{"servers":{"test":{"type":"in-memory","args":["--root","${WORKSPACE}"]}}}"#)
        else {
            panic!("expected in-memory server");
        };

        let expanded = config.expand(&Vars::new().with("WORKSPACE", "/workspace")).unwrap();

        assert_eq!(expanded.args, ["--root", "/workspace"]);
    }
}
