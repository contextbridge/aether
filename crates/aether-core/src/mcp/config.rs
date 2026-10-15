use super::ToolExposure;
use aether_auth::{SigV4Config, SigV4Credentials, SigV4Signer};
use http::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, InvalidHeaderName, InvalidHeaderValue};
use mcp_utils::client::{HttpAuth, OAuthClientRegistration, OAuthSettings, Transport};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU16;
use std::path::Path;
use std::time::Duration;
use utils::variables::{VarError, Vars};

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
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
    #[serde(rename = "type", default)]
    pub type_: StdioType,

    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(rename = "deferTools", default, skip_serializing_if = "is_model_visible")]
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
pub struct McpSigV4Config {
    pub service: String,
    pub region: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigv4: Option<McpSigV4Config>,

    #[serde(rename = "deferTools", default, skip_serializing_if = "is_model_visible")]
    pub defer_tools: ToolExposure,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InMemoryServerConfig {
    #[serde(rename = "type")]
    pub type_: InMemoryType,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(rename = "deferTools", default, skip_serializing_if = "is_model_visible")]
    pub defer_tools: ToolExposure,
}

#[derive(Debug)]
pub(super) enum ServerSource {
    Ready(Transport),
    InMemory(Vec<String>),
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

    #[error("A server can set at most one of `oauth`, `sigv4` or an `Authorization` header")]
    ConflictingAuth,
}

pub fn http_transport(url: String, headers: HeaderMap) -> Transport {
    let auth = default_auth(&headers);
    Transport::Http { url, headers, auth }
}

impl McpConfig {
    pub fn new(servers: BTreeMap<String, McpServerConfig>) -> Self {
        Self { servers }
    }

    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content)
    }

    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn defer_all_tools(&mut self) {
        for exposure in self.servers.values_mut().map(McpServerConfig::defer_tools_mut) {
            if !exposure.defers_tools() {
                *exposure = ToolExposure::deferred_all();
            }
        }
    }
}

/// Merges configs in order; a server named again replaces the earlier definition.
impl FromIterator<McpConfig> for McpConfig {
    fn from_iter<T: IntoIterator<Item = McpConfig>>(configs: T) -> Self {
        Self::new(configs.into_iter().flat_map(|config| config.servers).collect())
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

    pub(super) fn resolve(self, vars: &Vars, cwd: &Path) -> Result<ServerSource, ParseError> {
        Ok(match self {
            McpServerConfig::Stdio(config) => ServerSource::Ready(config.into_transport(vars, cwd)?),
            McpServerConfig::Remote(config) => ServerSource::Ready(config.into_transport(vars)?),
            McpServerConfig::InMemory(config) => ServerSource::InMemory(expand_all(vars, &config.args)?),
        })
    }

    fn defer_tools_mut(&mut self) -> &mut ToolExposure {
        match self {
            McpServerConfig::Stdio(config) => &mut config.defer_tools,
            McpServerConfig::Remote(config) => &mut config.defer_tools,
            McpServerConfig::InMemory(config) => &mut config.defer_tools,
        }
    }
}

impl StdioServerConfig {
    pub fn into_transport(self, vars: &Vars, cwd: &Path) -> Result<Transport, ParseError> {
        let env = self.env.into_iter().map(|(name, value)| Ok((name, vars.expand(&value)?)));
        Ok(Transport::Stdio {
            command: vars.expand(&self.command)?,
            args: expand_all(vars, &self.args)?,
            env: env.collect::<Result<_, VarError>>()?,
            cwd: Some(cwd.to_path_buf()),
        })
    }
}

impl McpOAuthConfig {
    fn into_settings(self) -> OAuthSettings {
        let callback_port = self.callback_port.map_or(AETHER_OAUTH_CALLBACK_PORT, NonZeroU16::get);
        let registration = match self {
            Self { client_id: Some(client_id), .. } => OAuthClientRegistration::PreRegistered(client_id),
            Self { client_metadata_url: Some(url), .. } => OAuthClientRegistration::ClientMetadata(url),
            Self { .. } => OAuthClientRegistration::ClientMetadata(AETHER_OAUTH_CLIENT_METADATA_URL.to_string()),
        };
        OAuthSettings { registration, callback_port }
    }
}

impl McpSigV4Config {
    fn into_signer(self, vars: &Vars) -> Result<SigV4Signer, VarError> {
        let profile = self.profile.map(|profile| vars.expand(&profile)).transpose()?;
        Ok(SigV4Signer::new(SigV4Config {
            service: vars.expand(&self.service)?,
            region: vars.expand(&self.region)?,
            credentials: SigV4Credentials::DefaultChain { profile },
            refresh_before_expiry: Duration::from_mins(5),
        }))
    }
}

impl RemoteServerConfig {
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
        let has_authorization = headers.contains_key(AUTHORIZATION);
        let auth = match (self.oauth, self.sigv4) {
            (Some(oauth), None) if !has_authorization => HttpAuth::OAuth(expand_oauth(vars, oauth)?.into_settings()),
            (None, Some(sigv4)) if !has_authorization => HttpAuth::SigV4(sigv4.into_signer(vars)?),
            (None, None) => default_auth(&headers),
            _ => return Err(ParseError::ConflictingAuth),
        };
        Ok(Transport::Http { url: vars.expand(&self.url)?, headers, auth })
    }
}

const AETHER_OAUTH_CLIENT_METADATA_URL: &str = "https://aether-agent.io/oauth/client-metadata.json";
const AETHER_OAUTH_CALLBACK_PORT: u16 = 3118;

fn is_model_visible(exposure: &ToolExposure) -> bool {
    !exposure.defers_tools()
}

fn expand_all(vars: &Vars, values: &[String]) -> Result<Vec<String>, VarError> {
    values.iter().map(|value| vars.expand(value)).collect()
}

fn default_auth(headers: &HeaderMap) -> HttpAuth {
    if headers.contains_key(AUTHORIZATION) {
        HttpAuth::None
    } else {
        HttpAuth::OAuth(McpOAuthConfig::default().into_settings())
    }
}

fn expand_oauth(vars: &Vars, oauth: McpOAuthConfig) -> Result<McpOAuthConfig, VarError> {
    let expand = |value: Option<String>| value.map(|value| vars.expand(&value)).transpose();
    Ok(McpOAuthConfig {
        client_id: expand(oauth.client_id)?,
        client_metadata_url: expand(oauth.client_metadata_url)?,
        callback_port: oauth.callback_port,
    })
}
