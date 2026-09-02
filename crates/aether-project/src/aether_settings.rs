use utils::SettingsStore;

use crate::agent_config::AgentConfig;
use crate::error::SettingsError;
use crate::{McpFileSpec, McpSourceSpec, PromptSource};
use aether_core::core::Prompt;
use llm::ProviderConnectionOverrides;
use mcp_utils::client::McpConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};
use utils::variables::{VarError, Vars};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CredentialsStoreConfig {
    /// Holds credentials in the OS keyring
    Keyring,

    /// Holds credentials in-memory and only for the lifetime of the
    /// process. Intended for tests and ephemeral runs that must not touch the OS
    /// keychain.
    Memory,

    /// Holds credentials in an encrypted file
    EncryptedFile {
        /// File path for the encrypted credential blob. Defaults to
        /// `.aether/credentials.enc` in the Aether home directory when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        /// Environment variable name to read the passphrase from. Uses
        /// `AETHER_CREDENTIALS_PASSWORD` when unset.
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "passwordEnv")]
        password_env: Option<String>,
    },
}

const PROJECT_SETTINGS_PATH: &str = ".aether/settings.json";
const USER_SETTINGS_FILENAME: &str = "settings.json";

pub fn user_settings_path() -> Option<PathBuf> {
    SettingsStore::new("AETHER_HOME", ".aether").map(|store| store.home().join(USER_SETTINGS_FILENAME))
}

pub fn user_settings_exist() -> bool {
    user_settings_path().is_some_and(|p| p.is_file())
}

pub fn project_settings_path(project_root: &Path) -> PathBuf {
    project_root.join(PROJECT_SETTINGS_PATH)
}

pub fn project_settings_exist(project_root: &Path) -> bool {
    project_settings_path(project_root).is_file()
}

/// Root that file-backed settings resources resolve against for the settings file at
/// `settings_path`: the project root (the parent of `.aether`) when the settings file lives in a
/// `.aether` directory, otherwise the settings file's directory.
pub fn settings_resource_root(settings_path: &Path) -> PathBuf {
    let Some(settings_dir) = settings_path.parent() else {
        return PathBuf::from(".");
    };

    if settings_dir.file_name().and_then(|name| name.to_str()) == Some(".aether") {
        return settings_dir.parent().unwrap_or(settings_dir).to_path_buf();
    }

    settings_dir.to_path_buf()
}

#[doc = include_str!("docs/aether_settings.md")]
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AetherSettings {
    /// Name of the agent to launch by default. Must match a `name` in `agents`.
    /// When unset, Aether falls back to the first user-invocable agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Default prompt sources shared by all agents. An agent inherits these only
    /// when its own `prompts` array is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<PromptSource>,
    /// Default MCP sources shared by all agents. An agent inherits these only when
    /// its own `mcps` array is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<McpSourceSpec>,
    /// Provider connection overrides (credentials, base URLs, inference profiles)
    /// applied to every agent unless overridden per-agent.
    #[serde(default, skip_serializing_if = "ProviderConnectionOverrides::is_empty")]
    pub providers: ProviderConnectionOverrides,
    /// Credential storage backend for OAuth tokens. Defaults to the OS keyring
    /// when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_store: Option<CredentialsStoreConfig>,
    /// OpenTelemetry `GenAI` telemetry configuration. Its presence enables telemetry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetrySettings>,
    /// The agents defined for this project. At least one agent is required.
    #[schemars(length(min = 1))]
    pub agents: Vec<AgentConfig>,
}

/// One settings layer's OpenTelemetry configuration. Every field is optional:
/// a field left unset inherits the value from lower-precedence settings layers
/// and falls back to its documented default when no layer sets it. Read
/// resolved values through the accessor methods.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetrySettings {
    /// `service.name` resource attribute. Defaults to `aether`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    /// Trace sampling ratio between 0.0 and 1.0. Defaults to 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_ratio: Option<f64>,
    /// Whether prompt, response, reasoning, and tool argument content is
    /// exported. Defaults to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_content: Option<bool>,
    /// Trace signal toggle. Enabled by default.
    #[serde(default, skip_serializing_if = "TelemetrySignalSettings::is_unset")]
    pub traces: TelemetrySignalSettings,
    /// Metric signal toggle. Enabled by default.
    #[serde(default, skip_serializing_if = "TelemetrySignalSettings::is_unset")]
    pub metrics: TelemetrySignalSettings,
    #[serde(default, skip_serializing_if = "OtlpTelemetrySettings::is_unset")]
    pub otlp: OtlpTelemetrySettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetrySignalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OtlpTelemetrySettings {
    /// Base URL for an OTLP/HTTP collector. Aether appends `/v1/traces` or
    /// `/v1/metrics` for the respective signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Exact OTLP/HTTP trace export URL. Overrides the traces URL derived from
    /// `endpoint`, for providers that require a signal-specific endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traces_endpoint: Option<String>,
    /// Exact OTLP/HTTP metric export URL. Overrides the metrics URL derived from
    /// `endpoint`, for providers that require a signal-specific endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl TelemetrySettings {
    pub fn effective_enabled(&self) -> bool {
        self.traces_enabled() || self.metrics_enabled()
    }

    pub fn service_name(&self) -> &str {
        self.service_name.as_deref().unwrap_or("aether")
    }

    pub fn sample_ratio(&self) -> f64 {
        self.sample_ratio.unwrap_or(1.0)
    }

    pub fn capture_content(&self) -> bool {
        self.capture_content.unwrap_or(false)
    }

    pub fn traces_enabled(&self) -> bool {
        self.traces.enabled.unwrap_or(true)
    }

    pub fn metrics_enabled(&self) -> bool {
        self.metrics.enabled.unwrap_or(true)
    }

    fn merge(&mut self, next: Self) {
        merge_field(&mut self.service_name, next.service_name);
        merge_field(&mut self.sample_ratio, next.sample_ratio);
        merge_field(&mut self.capture_content, next.capture_content);
        merge_field(&mut self.traces.enabled, next.traces.enabled);
        merge_field(&mut self.metrics.enabled, next.metrics.enabled);
        merge_field(&mut self.otlp.endpoint, next.otlp.endpoint);
        merge_field(&mut self.otlp.traces_endpoint, next.otlp.traces_endpoint);
        merge_field(&mut self.otlp.metrics_endpoint, next.otlp.metrics_endpoint);
        self.otlp.headers.extend(next.otlp.headers);
    }
}

impl TelemetrySignalSettings {
    fn is_unset(&self) -> bool {
        self.enabled.is_none()
    }
}

impl OtlpTelemetrySettings {
    pub fn resolved_headers(&self, vars: &Vars) -> Result<BTreeMap<String, String>, VarError> {
        self.headers.iter().map(|(name, value)| Ok((name.clone(), vars.expand(value)?))).collect()
    }

    fn is_unset(&self) -> bool {
        self.endpoint.is_none()
            && self.traces_endpoint.is_none()
            && self.metrics_endpoint.is_none()
            && self.headers.is_empty()
    }
}

fn merge_field<T>(current: &mut Option<T>, next: Option<T>) {
    if next.is_some() {
        *current = next;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsFileSource {
    pub path: PathBuf,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum AetherSettingsSource {
    File(SettingsFileSource),
    OptionalFile(SettingsFileSource),
    Json(String),
    Value(Box<AetherSettings>),
}

impl SettingsFileSource {
    pub fn new(path: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), root: root.into() }
    }
}

impl AetherSettings {
    pub fn load_default(project_root: &Path) -> Result<Self, SettingsError> {
        Self::load(project_root, default_sources(project_root))
    }

    pub fn load(
        project_root: &Path,
        sources: impl IntoIterator<Item = AetherSettingsSource>,
    ) -> Result<Self, SettingsError> {
        sources.into_iter().try_fold(Self::default(), |config, source| {
            let next = Self::load_source(project_root, source)?;
            Ok(config.merge(next))
        })
    }

    pub fn load_file_for_export(path: &Path) -> Result<Self, SettingsError> {
        let content = read_to_string(path).map_err(|source| {
            SettingsError::IoError(format!("failed to read settings file '{}': {source}", path.display()))
        })?;
        let mut settings = Self::try_from(content.as_str())?;
        settings.inline_resources(&settings_resource_root(path))?;
        Ok(settings)
    }

    pub fn merge(mut self, next: Self) -> Self {
        if next.agent.is_some() {
            self.agent = next.agent;
        }

        if !next.prompts.is_empty() {
            self.prompts = next.prompts;
        }
        if !next.mcps.is_empty() {
            self.mcps = next.mcps;
        }
        self.providers.merge(next.providers);

        if next.credentials_store.is_some() {
            self.credentials_store = next.credentials_store;
        }

        if let Some(next_telemetry) = next.telemetry {
            self.telemetry.get_or_insert_default().merge(next_telemetry);
        }

        for next_agent in next.agents {
            if let Some(existing) = self.agents.iter_mut().find(|agent| agent.name.trim() == next_agent.name.trim()) {
                *existing = next_agent;
            } else {
                self.agents.push(next_agent);
            }
        }

        self
    }

    /// Replace every file- and glob-backed prompt and MCP source with its
    /// inlined contents, resolving paths against `root`.
    ///
    /// The result serializes to a self-contained settings document with no
    /// external file references, suitable for shipping to a machine or
    /// container that does not have the authoring repository mounted.
    pub fn inline_resources(&mut self, root: &Path) -> Result<(), SettingsError> {
        self.prompts = inline_prompt_sources(&self.prompts, root)?;
        self.mcps = inline_mcp_sources(&self.mcps, root)?;
        for agent in &mut self.agents {
            agent.prompts = inline_prompt_sources(&agent.prompts, root)?;
            agent.mcps = inline_mcp_sources(&agent.mcps, root)?;
        }
        Ok(())
    }

    fn load_source(project_root: &Path, source: AetherSettingsSource) -> Result<Self, SettingsError> {
        match source {
            AetherSettingsSource::File(source) => load_file_source(project_root, source, false),
            AetherSettingsSource::OptionalFile(source) => load_file_source(project_root, source, true),
            AetherSettingsSource::Json(json) => Self::try_from(json.as_str()),
            AetherSettingsSource::Value(settings) => Ok(*settings),
        }
    }
}

fn default_sources(project_root: &Path) -> Vec<AetherSettingsSource> {
    let aether_home = SettingsStore::new("AETHER_HOME", ".aether").map(|store| store.home().to_path_buf());
    default_sources_for_home(project_root, aether_home.as_deref())
}

fn default_sources_for_home(project_root: &Path, aether_home: Option<&Path>) -> Vec<AetherSettingsSource> {
    let mut sources = Vec::new();
    if let Some(aether_home) = aether_home {
        sources.push(AetherSettingsSource::OptionalFile(SettingsFileSource::new("settings.json", aether_home)));
    }
    sources.push(AetherSettingsSource::OptionalFile(SettingsFileSource::new(PROJECT_SETTINGS_PATH, project_root)));
    sources
}

fn load_file_source(
    project_root: &Path,
    source: SettingsFileSource,
    missing_is_empty: bool,
) -> Result<AetherSettings, SettingsError> {
    let root = resolve_against(project_root, source.root);
    let path = resolve_against(&root, source.path);
    let settings = load_file(&path, missing_is_empty)?;
    let source_root = (root != project_root).then_some(root.as_path());
    Ok(normalize_resource_paths(settings, source_root))
}

fn resolve_against(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { base.join(path) }
}

fn load_file(path: &Path, missing_is_empty: bool) -> Result<AetherSettings, SettingsError> {
    match read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(AetherSettings::default()),
        Ok(content) => AetherSettings::try_from(content.as_str()),
        Err(error) if missing_is_empty && error.kind() == std::io::ErrorKind::NotFound => Ok(AetherSettings::default()),
        Err(error) => Err(SettingsError::IoError(format!("Failed to read {}: {}", path.display(), error))),
    }
}

fn normalize_resource_paths(mut settings: AetherSettings, source_root: Option<&Path>) -> AetherSettings {
    let Some(root) = source_root else { return settings };
    promote_prompt_sources(&mut settings.prompts, root);
    promote_mcp_sources(&mut settings.mcps, root);

    for agent in &mut settings.agents {
        promote_prompt_sources(&mut agent.prompts, root);
        promote_mcp_sources(&mut agent.mcps, root);
    }

    settings
}

fn inline_prompt_sources(sources: &[PromptSource], root: &Path) -> Result<Vec<PromptSource>, SettingsError> {
    let mut inlined = Vec::new();
    for prompt in Prompt::from_sources(root, sources)? {
        let text = match prompt {
            Prompt::Text(text) => text,
            Prompt::File { path, .. } => read_to_string(&path)
                .map_err(|e| SettingsError::IoError(format!("Failed to read prompt '{}': {e}", path.display())))?,
            Prompt::McpInstructions(_) => continue,
        };
        inlined.push(PromptSource::Text { text });
    }
    Ok(inlined)
}

fn inline_mcp_sources(sources: &[McpSourceSpec], root: &Path) -> Result<Vec<McpSourceSpec>, SettingsError> {
    let mut inlined = Vec::new();
    for source in sources {
        let McpSourceSpec::File(McpFileSpec { path, defer_tools, optional }) = source else {
            inlined.push(source.clone());
            continue;
        };

        let full_path = match path.resolve(root) {
            Ok(full_path) => full_path,
            Err(VarError::NotFound(variable)) => {
                if *optional {
                    tracing::warn!(
                        "Skipping optional MCP config '{}': variable '{variable}' is not defined",
                        path.as_authored()
                    );
                    continue;
                }
                return Err(SettingsError::UnresolvedMcpConfigVariable {
                    path: path.as_authored().to_string(),
                    variable,
                });
            }
        };

        if !full_path.is_file() {
            if *optional {
                continue;
            }
            return Err(SettingsError::InvalidMcpConfigPath { path: path.as_authored().to_string() });
        }

        let mut config = McpConfig::from_json_file(&full_path)
            .map_err(|e| SettingsError::IoError(format!("Failed to read MCP config '{}': {e}", full_path.display())))?;
        if *defer_tools {
            config.defer_all_tools();
        }
        inlined.push(McpSourceSpec::Inline { servers: config.servers });
    }
    Ok(inlined)
}

fn promote_prompt_sources(sources: &mut [PromptSource], source_root: &Path) {
    for source in sources {
        match source {
            PromptSource::File { path, .. } | PromptSource::Glob { pattern: path, .. } => {
                path.promote_relative(source_root);
            }
            PromptSource::Text { .. } => {}
        }
    }
}

fn promote_mcp_sources(sources: &mut [McpSourceSpec], source_root: &Path) {
    for source in sources {
        if let McpSourceSpec::File(file) = source {
            file.path.promote_relative(source_root);
        }
    }
}

impl TryFrom<&str> for AetherSettings {
    type Error = SettingsError;

    fn try_from(content: &str) -> Result<Self, Self::Error> {
        serde_json::from_str(content).map_err(|e| SettingsError::ParseError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentCatalog, McpFileSpec, McpSourceSpec, PromptSource};
    use aether_core::agent_spec::McpConfigSource;
    use aether_core::core::Prompt;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs::{create_dir_all, write};

    #[test]
    fn telemetry_is_disabled_when_absent() {
        assert!(AetherSettings::default().telemetry.is_none());

        let settings = SettingsJson::new().field("telemetry", json!({})).parse();
        assert!(settings.telemetry.unwrap().effective_enabled());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_telemetry_camel_case_and_http_protobuf() {
        let config = SettingsJson::new()
            .field(
                "telemetry",
                json!({
                    "serviceName": "aether-test",
                    "sampleRatio": 0.5,
                    "captureContent": true,
                    "traces": { "enabled": true },
                    "metrics": { "enabled": false },
                    "otlp": {
                        "endpoint": "http://localhost:4318",
                        "headers": { "authorization": "Bearer token" }
                    }
                }),
            )
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        let telemetry = config.telemetry.as_ref().unwrap();
        assert_eq!(telemetry.service_name(), "aether-test");
        assert_eq!(telemetry.sample_ratio(), 0.5);
        assert!(telemetry.capture_content());
        assert!(telemetry.traces_enabled());
        assert!(!telemetry.metrics_enabled());
        assert_eq!(telemetry.otlp.headers.get("authorization").map(String::as_str), Some("Bearer token"));
    }

    #[test]
    fn telemetry_with_no_enabled_signals_does_not_require_endpoint() {
        let config = SettingsJson::new()
            .field("telemetry", json!({ "traces": { "enabled": false }, "metrics": { "enabled": false } }))
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        assert!(!config.telemetry.unwrap().effective_enabled());
    }

    #[test]
    fn telemetry_overlays_merge_fields_and_allow_explicit_defaults() {
        let config = AetherSettings::load(
            Path::new("/project"),
            [
                AetherSettingsSource::Json(
                    SettingsJson::new()
                        .field(
                            "telemetry",
                            json!({
                                "captureContent": true,
                                "traces": { "enabled": true },
                                "metrics": { "enabled": true },
                                "otlp": {
                                    "endpoint": "http://localhost:4318",
                                    "tracesEndpoint": "https://traces.example.com/export",
                                    "metricsEndpoint": "https://metrics.example.com/export"
                                }
                            }),
                        )
                        .build(),
                ),
                AetherSettingsSource::Json(
                    SettingsJson::new()
                        .field(
                            "telemetry",
                            json!({
                                "captureContent": false,
                                "metrics": { "enabled": false }
                            }),
                        )
                        .build(),
                ),
            ],
        )
        .unwrap();

        let telemetry = config.telemetry.unwrap();
        assert!(!telemetry.capture_content(), "explicit default false remains distinguishable from omission");
        assert!(telemetry.traces_enabled(), "omitted nested fields remain inherited");
        assert!(!telemetry.metrics_enabled(), "nested overrides merge independently");
        assert_eq!(telemetry.otlp.endpoint.as_deref(), Some("http://localhost:4318"));
        assert_eq!(telemetry.otlp.traces_endpoint.as_deref(), Some("https://traces.example.com/export"));
        assert_eq!(telemetry.otlp.metrics_endpoint.as_deref(), Some("https://metrics.example.com/export"));
    }

    #[test]
    fn telemetry_overlay_merges_otlp_headers_per_key() {
        let config = AetherSettings::load(
            Path::new("/project"),
            [
                AetherSettingsSource::Json(
                    SettingsJson::new()
                        .field(
                            "telemetry",
                            json!({
                                "otlp": {
                                    "endpoint": "http://localhost:4318",
                                    "headers": { "authorization": "Bearer base", "x-base": "1" }
                                }
                            }),
                        )
                        .build(),
                ),
                AetherSettingsSource::Json(
                    SettingsJson::new()
                        .field(
                            "telemetry",
                            json!({
                                "otlp": {
                                    "headers": { "authorization": "Bearer overlay", "x-overlay": "2" }
                                }
                            }),
                        )
                        .build(),
                ),
            ],
        )
        .unwrap();

        let headers = config.telemetry.unwrap().otlp.headers;
        assert_eq!(headers.get("authorization").map(String::as_str), Some("Bearer overlay"));
        assert_eq!(headers.get("x-base").map(String::as_str), Some("1"), "base-layer headers survive an overlay");
        assert_eq!(headers.get("x-overlay").map(String::as_str), Some("2"));
    }

    #[test]
    fn telemetry_overlay_inherits_unspecified_parent_fields() {
        let config = AetherSettings::load(
            Path::new("/project"),
            [
                AetherSettingsSource::Json(
                    SettingsJson::new()
                        .field(
                            "telemetry",
                            json!({
                                "sampleRatio": 0.25,
                                "otlp": { "endpoint": "http://localhost:4318" }
                            }),
                        )
                        .build(),
                ),
                AetherSettingsSource::Json(
                    SettingsJson::new().field("telemetry", json!({ "captureContent": true })).build(),
                ),
            ],
        )
        .unwrap();

        let telemetry = config.telemetry.unwrap();
        assert!(telemetry.capture_content());
        assert!((telemetry.sample_ratio() - 0.25).abs() < f64::EPSILON);
        assert_eq!(telemetry.otlp.endpoint.as_deref(), Some("http://localhost:4318"));
    }

    #[test]
    fn project_settings_path_points_at_project_aether_settings() {
        assert_eq!(project_settings_path(Path::new("/repo")), PathBuf::from("/repo/.aether/settings.json"));
    }

    #[test]
    fn settings_resource_root_uses_project_root_for_aether_dir_settings() {
        assert_eq!(settings_resource_root(Path::new("/repo/.aether/settings.json")), PathBuf::from("/repo"));
        assert_eq!(settings_resource_root(Path::new("/repo/config/settings.json")), PathBuf::from("/repo/config"));
    }

    #[test]
    fn project_settings_exist_checks_project_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!project_settings_exist(dir.path()));
        write_file(dir.path(), PROJECT_SETTINGS_PATH, "{}");
        assert!(project_settings_exist(dir.path()));
    }

    #[test]
    fn resolves_selected_agent() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        let config = AetherSettings {
            agent: Some("beta".to_string()),
            agents: vec![agent_config("alpha"), agent_config("beta")],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();

        assert_eq!(catalog.default_agent().map(|spec| spec.name.as_str()), Some("beta"));
    }

    #[test]
    fn rejects_selected_agent_that_is_not_user_invocable() {
        let mut internal = agent_config("internal");
        internal.user_invocable = false;
        internal.agent_invocable = true;
        let config =
            AetherSettings { agent: Some("internal".to_string()), agents: vec![internal], ..AetherSettings::default() };

        let err = AgentCatalog::from_settings(Path::new("/tmp"), config).unwrap_err();

        assert!(matches!(err, SettingsError::NonUserInvocableAgentSelector { .. }));
    }

    #[test]
    fn settings_file_paths_are_project_relative() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        write_file(
            dir.path(),
            "nested/config.json",
            &SettingsJson::new()
                .agent(
                    AgentJson::new("alpha", "Alpha").field("prompts", json!([{ "type": "file", "path": "PROMPT.md" }])),
                )
                .build(),
        );

        let config = AetherSettings::load(
            dir.path(),
            [AetherSettingsSource::File(SettingsFileSource::new("nested/config.json", dir.path()))],
        )
        .unwrap();
        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();

        assert_eq!(catalog.all()[0].name, "alpha");
    }

    #[test]
    fn load_merges_sources_with_rightmost_agent_winning() {
        let dir = tempfile::tempdir().unwrap();
        let base = AetherSettings {
            agent: Some("alpha".to_string()),
            prompts: vec![PromptSource::file("BASE.md")],
            agents: vec![AgentConfig { description: "Base alpha".to_string(), ..agent_config("alpha") }],
            ..AetherSettings::default()
        };
        let override_config = AetherSettings {
            agent: Some("beta".to_string()),
            prompts: vec![PromptSource::file("OVERRIDE.md")],
            agents: vec![
                AgentConfig { description: "Override alpha".to_string(), ..agent_config("alpha") },
                agent_config("beta"),
            ],
            ..AetherSettings::default()
        };

        let config = AetherSettings::load(
            dir.path(),
            [AetherSettingsSource::Value(Box::new(base)), AetherSettingsSource::Value(Box::new(override_config))],
        )
        .unwrap();

        assert_eq!(
            config,
            AetherSettings {
                agent: Some("beta".to_string()),
                prompts: vec![PromptSource::file("OVERRIDE.md")],
                agents: vec![
                    AgentConfig { description: "Override alpha".to_string(), ..agent_config("alpha") },
                    agent_config("beta"),
                ],
                ..AetherSettings::default()
            }
        );
    }

    #[test]
    fn load_default_merges_user_and_project_settings_with_project_winning() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(
            &aether_home,
            "settings.json",
            &SettingsJson::new()
                .field("agent", "shared")
                .field("prompts", json!(["USER.md"]))
                .agent(AgentJson::new("shared", "User shared"))
                .agent(AgentJson::new("user-only", "User only"))
                .build(),
        );
        write_file(
            project.path(),
            ".aether/settings.json",
            &SettingsJson::new()
                .field("agent", "project-only")
                .field("prompts", json!(["PROJECT.md"]))
                .agent(AgentJson::new("shared", "Project shared"))
                .agent(AgentJson::new("project-only", "Project only"))
                .build(),
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        assert_eq!(
            config,
            AetherSettings {
                agent: Some("project-only".to_string()),
                prompts: vec![PromptSource::file("PROJECT.md")],
                agents: vec![
                    settings_agent("shared", "Project shared"),
                    settings_agent("user-only", "User only"),
                    settings_agent("project-only", "Project only"),
                ],
                ..AetherSettings::default()
            }
        );
    }

    #[test]
    fn load_default_uses_user_settings_when_project_settings_are_missing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(
            &aether_home,
            "settings.json",
            &SettingsJson::new().agent(AgentJson::new("user-only", "User only")).build(),
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        assert_eq!(
            config,
            AetherSettings { agents: vec![settings_agent("user-only", "User only")], ..AetherSettings::default() }
        );
    }

    #[test]
    fn load_default_resolves_user_agent_paths_from_aether_home() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(&aether_home, "agents/user.md", "User instructions");
        write_file(&aether_home, "mcp/user.json", r#"{"servers":{}}"#);
        write_file(
            &aether_home,
            "settings.json",
            &SettingsJson::new()
                .agent(
                    AgentJson::new("user-only", "User only")
                        .field("prompts", json!(["agents/user.md"]))
                        .field("mcps", json!(["mcp/user.json"])),
                )
                .build(),
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("user-only").unwrap();

        let expected_prompt = aether_home.join("agents/user.md");
        assert!(spec.prompts.iter().any(|prompt| match prompt {
            Prompt::File { path, .. } => path == &expected_prompt,
            Prompt::Text(_) | Prompt::McpInstructions(_) => false,
        }));
        assert!(matches!(
            &spec.mcp_config_sources[0],
            McpConfigSource::File { path, defer_tools: false } if path == &aether_home.join("mcp/user.json")
        ));
    }

    #[test]
    fn load_default_uses_project_settings_when_user_settings_are_missing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(
            project.path(),
            ".aether/settings.json",
            &SettingsJson::new().agent(AgentJson::new("project-only", "Project only")).build(),
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();

        assert_eq!(
            config,
            AetherSettings {
                agents: vec![settings_agent("project-only", "Project only")],
                ..AetherSettings::default()
            }
        );
    }

    #[test]
    fn load_default_returns_default_when_user_and_project_settings_are_missing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        assert_eq!(config, AetherSettings::default());
    }

    #[test]
    fn load_default_rejects_malformed_user_settings() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(&aether_home, "settings.json", "{not-json");
        let err = load_default_from_home(project.path(), &aether_home).unwrap_err();
        assert!(matches!(err, SettingsError::ParseError(_)));
    }

    #[test]
    fn strict_file_source_errors_when_missing() {
        let project = tempfile::tempdir().unwrap();
        let err = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::File(SettingsFileSource::new("missing.json", project.path()))],
        )
        .unwrap_err();

        assert!(matches!(err, SettingsError::IoError(_)));
    }

    #[test]
    fn optional_file_source_returns_default_when_missing() {
        let project = tempfile::tempdir().unwrap();
        let config = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::OptionalFile(SettingsFileSource::new("missing.json", project.path()))],
        )
        .unwrap();

        assert_eq!(config, AetherSettings::default());
    }

    #[test]
    fn inline_resources_replaces_file_sources_with_their_contents() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "BASE.md", "Be helpful");
        write_file(root.path(), "AGENT.md", "Edit carefully");
        write_file(root.path(), "mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"run"}}}"#);

        let mut settings = AetherSettings {
            prompts: vec![PromptSource::file("BASE.md")],
            mcps: vec![McpSourceSpec::file("mcp.json")],
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("AGENT.md")],
                mcps: vec![McpSourceSpec::file("mcp.json")],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        settings.inline_resources(root.path()).unwrap();

        assert_eq!(settings.prompts, vec![PromptSource::Text { text: "Be helpful".to_string() }]);
        assert_eq!(settings.agents[0].prompts, vec![PromptSource::Text { text: "Edit carefully".to_string() }]);
        assert!(matches!(&settings.mcps[0], McpSourceSpec::Inline { servers } if servers.contains_key("coding")));
        assert!(
            matches!(&settings.agents[0].mcps[0], McpSourceSpec::Inline { servers } if servers.contains_key("coding"))
        );

        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("BASE.md") && !serialized.contains("mcp.json"), "{serialized}");
    }

    #[test]
    fn inline_resources_drops_optional_missing_sources() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "PROMPT.md", "Agent prompt");
        let mut settings = AetherSettings {
            prompts: vec![PromptSource::file("absent.md").optional()],
            mcps: vec![McpSourceSpec::File(McpFileSpec::new("absent.json").optional())],
            agents: vec![agent_config("alpha")],
            ..AetherSettings::default()
        };

        settings.inline_resources(root.path()).unwrap();

        assert!(settings.prompts.is_empty());
        assert!(settings.mcps.is_empty());
    }

    #[test]
    fn inline_resources_errors_on_required_missing_mcp() {
        let root = tempfile::tempdir().unwrap();
        let mut settings = AetherSettings {
            mcps: vec![McpSourceSpec::file("absent.json")],
            agents: vec![agent_config("alpha")],
            ..AetherSettings::default()
        };

        let err = settings.inline_resources(root.path()).unwrap_err();
        assert!(matches!(err, SettingsError::InvalidMcpConfigPath { .. }));
    }

    #[test]
    fn resolves_inline_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        let config = AetherSettings {
            agent: None,
            agents: vec![AgentConfig {
                mcps: vec![McpSourceSpec::Inline { servers: BTreeMap::new() }],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert_eq!(spec.mcp_config_sources.len(), 1);
        assert!(matches!(spec.mcp_config_sources[0], McpConfigSource::Inline(_)));
    }

    #[test]
    fn parses_top_level_prompt_and_mcp_defaults() {
        let config = SettingsJson::new()
            .field("prompts", json!([{ "type": "file", "path": "BASE.md" }]))
            .field("mcps", json!([{ "type": "file", "path": "mcp.json" }]))
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        assert_eq!(
            config,
            AetherSettings {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpSourceSpec::file("mcp.json")],
                agents: vec![settings_agent("alpha", "Alpha")],
                ..AetherSettings::default()
            }
        );
    }

    #[test]
    fn parses_and_serializes_string_shorthand_for_file_sources() {
        let config = SettingsJson::new()
            .field("prompts", json!(["BASE.md"]))
            .field("mcps", json!(["mcp.json"]))
            .agent(
                AgentJson::new("alpha", "Alpha")
                    .field("prompts", json!(["AGENT.md"]))
                    .field("mcps", json!(["agent-mcp.json"])),
            )
            .parse();

        assert_eq!(
            config,
            AetherSettings {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpSourceSpec::file("mcp.json")],
                agents: vec![AgentConfig {
                    prompts: vec![PromptSource::file("AGENT.md")],
                    mcps: vec![McpSourceSpec::file("agent-mcp.json")],
                    ..settings_agent("alpha", "Alpha")
                }],
                ..AetherSettings::default()
            }
        );

        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["prompts"], serde_json::json!(["BASE.md"]));
        assert_eq!(value["mcps"], serde_json::json!(["mcp.json"]));
        assert_eq!(value["agents"][0]["prompts"], serde_json::json!(["AGENT.md"]));
        assert_eq!(value["agents"][0]["mcps"], serde_json::json!(["agent-mcp.json"]));
    }

    #[test]
    fn deserializes_legacy_proxy_mcp_file_as_deferred() {
        let source: McpSourceSpec =
            serde_json::from_value(serde_json::json!({"type":"file", "path":"mcp.json", "proxy":true})).unwrap();

        assert!(matches!(source, McpSourceSpec::File(McpFileSpec { defer_tools: true, .. })));
    }

    #[test]
    fn serializes_deferred_mcp_file_as_typed_object() {
        let source: McpSourceSpec = McpFileSpec::new("mcp.json").defer_tools().into();

        let value = serde_json::to_value(source).unwrap();

        assert_eq!(value, serde_json::json!({"type":"file", "path":"mcp.json", "deferTools":true}));
    }

    #[test]
    fn rejects_old_top_level_mcp_servers_field() {
        let err = AetherSettings::try_from(
            SettingsJson::new()
                .field("mcpServers", json!(["mcp.json"]))
                .agent(
                    AgentJson::new("alpha", "Alpha").field("prompts", json!([{ "type": "file", "path": "PROMPT.md" }])),
                )
                .build()
                .as_str(),
        )
        .unwrap_err();

        assert!(matches!(err, SettingsError::ParseError(message) if message.contains("mcpServers")));
    }

    #[test]
    fn load_default_resolves_workspace_scoped_user_prompt_and_mcp_paths() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(&aether_home, "agents/planner/SYSTEM.md", "System instructions");
        write_file(project.path(), "AGENTS.md", "Agent instructions");
        write_file(project.path(), ".aether/mcp.json", r#"{"servers":{}}"#);
        write_file(
            &aether_home,
            "settings.json",
            &SettingsJson::new()
                .agent(
                    AgentJson::new("planner", "Plans work")
                        .field(
                            "prompts",
                            json!([
                                "agents/planner/SYSTEM.md",
                                { "type": "file", "path": "${WORKSPACE}/AGENTS.md" }
                            ]),
                        )
                        .field("mcps", json!([{ "type": "file", "path": "${WORKSPACE}/.aether/mcp.json" }])),
                )
                .build(),
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        let expected_system = aether_home.join("agents/planner/SYSTEM.md");
        let expected_agents = project.path().join("AGENTS.md");
        assert!(spec.prompts.iter().any(|p| match p {
            Prompt::File { path, .. } => path == &expected_system,
            _ => false,
        }));
        assert!(spec.prompts.iter().any(|p| match p {
            Prompt::File { path, .. } => path == &expected_agents,
            _ => false,
        }));
        assert!(matches!(
            &spec.mcp_config_sources[0],
            McpConfigSource::File { path, defer_tools: false } if *path == project.path().join(".aether/mcp.json")
        ));
    }

    #[test]
    fn workspace_scoped_paths_expand_in_project_settings_without_absolutizing_normal_relative_paths() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "PROJECT.md", "Project prompt");
        write_file(project.path(), "AGENTS.md", "Agent prompt");
        write_file(
            project.path(),
            ".aether/settings.json",
            &SettingsJson::new()
                .agent(
                    AgentJson::new("alpha", "Alpha")
                        .field("prompts", json!(["PROJECT.md", { "type": "file", "path": "${WORKSPACE}/AGENTS.md" }])),
                )
                .build(),
        );

        let config = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::OptionalFile(SettingsFileSource::new(PROJECT_SETTINGS_PATH, project.path()))],
        )
        .unwrap();

        assert_eq!(config.agents[0].prompts[0], PromptSource::file("PROJECT.md"));
        assert_eq!(config.agents[0].prompts[1], PromptSource::file("${WORKSPACE}/AGENTS.md"));
    }

    #[test]
    fn json_and_value_sources_preserve_workspace_scoped_paths_losslessly() {
        let project = tempfile::tempdir().unwrap();

        let json_config = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::Json(
                SettingsJson::new()
                    .agent(AgentJson::new("alpha", "Alpha").field("prompts", json!(["${WORKSPACE}/AGENTS.md"])))
                    .build(),
            )],
        )
        .unwrap();

        assert_eq!(json_config.agents[0].prompts[0], PromptSource::file("${WORKSPACE}/AGENTS.md"));

        let value_config = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::Value(Box::new(AetherSettings {
                agents: vec![AgentConfig {
                    prompts: vec![PromptSource::file("${WORKSPACE}/AGENTS.md")],
                    ..agent_config("alpha")
                }],
                ..AetherSettings::default()
            }))],
        )
        .unwrap();
        assert_eq!(value_config.agents[0].prompts[0], PromptSource::file("${WORKSPACE}/AGENTS.md"));
    }

    #[test]
    fn optional_workspace_scoped_mcp_source_is_skipped_when_missing() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpFileSpec::new("${WORKSPACE}/.aether/mcp.json").optional().into()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let config = AetherSettings::load(project.path(), [AetherSettingsSource::Value(Box::new(config))]).unwrap();
        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert!(spec.mcp_config_sources.is_empty());
    }

    #[test]
    fn optional_mcp_source_skips_unresolved_variable() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpFileSpec::new("${DEFINITELY_NOT_SET_VAR_MCP_OPTIONAL}/mcp.json").optional().into()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let config = AetherSettings::load(project.path(), [AetherSettingsSource::Value(Box::new(config))]).unwrap();
        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert!(spec.mcp_config_sources.is_empty());
    }

    #[test]
    fn required_mcp_source_errors_on_unresolved_variable() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpSourceSpec::file("${DEFINITELY_NOT_SET_VAR_MCP_REQ}/mcp.json")],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(project.path(), config).unwrap_err();
        assert!(matches!(err, SettingsError::UnresolvedMcpConfigVariable { .. }));
    }

    #[test]
    fn required_workspace_scoped_mcp_source_errors_when_missing() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpSourceSpec::file("nonexistent.json")],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(project.path(), config).unwrap_err();
        assert!(matches!(err, SettingsError::InvalidMcpConfigPath { .. }));
    }

    #[test]
    fn optional_existing_mcp_source_preserves_defer_tools_flag() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        write_file(project.path(), "mcp.json", r#"{"servers":{}}"#);
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpFileSpec::new("mcp.json").defer_tools().optional().into()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert!(matches!(&spec.mcp_config_sources[0], McpConfigSource::File { defer_tools: true, .. }));
    }

    #[test]
    fn optional_mcp_source_serializes_as_typed_object() {
        let source: McpSourceSpec = McpFileSpec::new("${WORKSPACE}/.aether/mcp.json").optional().into();
        let value = serde_json::to_value(source).unwrap();
        assert_eq!(value, serde_json::json!({"type":"file", "path":"${WORKSPACE}/.aether/mcp.json", "optional":true}));
    }

    #[test]
    fn optional_prompt_source_serializes_as_typed_object() {
        let source = PromptSource::file("${WORKSPACE}/AGENTS.md").optional();
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(value, serde_json::json!({"type":"file", "path":"${WORKSPACE}/AGENTS.md", "optional":true}));
    }

    #[test]
    fn all_optional_prompts_missing_errors_with_no_prompts() {
        let project = tempfile::tempdir().unwrap();
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("MISSING.md").optional()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(project.path(), config).unwrap_err();
        assert!(matches!(err, SettingsError::AllOptionalPromptsMissing { agent } if agent == "alpha"));
    }

    #[test]
    fn settings_round_trip_preserves_workspace_prefix_and_relative_paths() {
        let original = SettingsJson::new()
            .agent(
                AgentJson::new("alpha", "Alpha")
                    .field(
                        "prompts",
                        json!([
                            "AGENTS.md",
                            "${WORKSPACE}/SYSTEM.md",
                            { "type": "file", "path": "${WORKSPACE}/.aether/rules.md", "optional": true },
                            { "type": "glob", "pattern": "${WORKSPACE}/.aether/rules/*.md" }
                        ]),
                    )
                    .field(
                        "mcps",
                        json!([
                            "mcp.json",
                            { "type": "file", "path": "${WORKSPACE}/.aether/mcp.json", "optional": true }
                        ]),
                    ),
            )
            .build();

        let settings = AetherSettings::try_from(original.as_str()).unwrap();
        let reserialized = serde_json::to_string(&settings).unwrap();
        let reparsed = AetherSettings::try_from(reserialized.as_str()).unwrap();

        assert_eq!(settings, reparsed, "settings should round-trip losslessly through serde");
    }

    #[test]
    fn user_settings_relative_paths_absolutize_at_load_but_workspace_token_is_preserved() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(&aether_home, "agents/planner/SYSTEM.md", "system");
        write_file(project.path(), "AGENTS.md", "agents");
        write_file(
            &aether_home,
            "settings.json",
            &SettingsJson::new()
                .agent(
                    AgentJson::new("planner", "Plans")
                        .field("prompts", json!(["agents/planner/SYSTEM.md", "${WORKSPACE}/AGENTS.md"])),
                )
                .build(),
        );

        let settings = load_default_from_home(project.path(), &aether_home).unwrap();

        let expected_user = aether_home.join("agents/planner/SYSTEM.md").to_string_lossy().to_string();
        assert_eq!(
            settings.agents[0].prompts,
            vec![PromptSource::file(expected_user), PromptSource::file("${WORKSPACE}/AGENTS.md")],
            "user-rooted relative paths must absolutize; ${{WORKSPACE}}/ paths must be preserved",
        );
    }

    fn load_default_from_home(project_root: &Path, aether_home: &Path) -> Result<AetherSettings, SettingsError> {
        AetherSettings::load(
            project_root,
            [
                AetherSettingsSource::OptionalFile(SettingsFileSource::new("settings.json", aether_home)),
                AetherSettingsSource::OptionalFile(SettingsFileSource::new(PROJECT_SETTINGS_PATH, project_root)),
            ],
        )
    }

    fn write_file(dir: &Path, path: &str, content: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            create_dir_all(parent).unwrap();
        }

        write(full, content).unwrap();
    }

    fn settings_agent(name: &str, description: &str) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            description: description.to_string(),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            user_invocable: true,
            ..AgentConfig::default()
        }
    }

    fn agent_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            description: format!("{name} agent"),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            user_invocable: true,
            prompts: vec![PromptSource::file("PROMPT.md")],
            ..AgentConfig::default()
        }
    }

    /// JSON counterpart of `settings_agent`: the agent boilerplate every settings
    /// document must carry, with per-test fields layered on top.
    struct AgentJson {
        value: serde_json::Value,
    }

    impl AgentJson {
        fn new(name: &str, description: &str) -> Self {
            Self {
                value: json!({
                    "name": name,
                    "description": description,
                    "model": "anthropic:claude-sonnet-4-5",
                    "userInvocable": true,
                }),
            }
        }

        fn field(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
            self.value[key] = value.into();
            self
        }
    }

    /// Builds the settings documents parsed in these tests, seeding the required
    /// `agents` array so tests only declare the fields they exercise.
    struct SettingsJson {
        document: serde_json::Value,
    }

    impl Default for SettingsJson {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SettingsJson {
        fn new() -> Self {
            Self { document: json!({ "agents": [] }) }
        }

        fn field(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
            self.document[key] = value.into();
            self
        }

        fn agent(mut self, agent: AgentJson) -> Self {
            self.document["agents"].as_array_mut().unwrap().push(agent.value);
            self
        }

        fn build(self) -> String {
            self.document.to_string()
        }

        fn parse(self) -> AetherSettings {
            let json = self.build();
            AetherSettings::try_from(json.as_str())
                .unwrap_or_else(|error| panic!("invalid settings JSON `{json}`: {error}"))
        }
    }

    #[test]
    fn parses_credentials_store_keyring() {
        let config = SettingsJson::new()
            .field("credentialsStore", json!({ "type": "keyring" }))
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        assert_eq!(config.credentials_store, Some(CredentialsStoreConfig::Keyring));
    }

    #[test]
    fn parses_credentials_store_memory() {
        let config = SettingsJson::new()
            .field("credentialsStore", json!({ "type": "memory" }))
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        assert_eq!(config.credentials_store, Some(CredentialsStoreConfig::Memory));
    }

    #[test]
    fn expands_otlp_header_environment_variables() {
        let settings = OtlpTelemetrySettings {
            headers: BTreeMap::from([
                ("authorization".to_string(), "Bearer $POSTHOG_PROJECT_TOKEN".to_string()),
                ("x-project".to_string(), "${POSTHOG_PROJECT_TOKEN}".to_string()),
                ("x-literal".to_string(), "$$POSTHOG_PROJECT_TOKEN".to_string()),
            ]),
            ..OtlpTelemetrySettings::default()
        };
        let vars = Vars::new().with_env_lookup(|name| (name == "POSTHOG_PROJECT_TOKEN").then(|| "token-value".into()));

        assert_eq!(
            settings.resolved_headers(&vars).unwrap(),
            BTreeMap::from([
                ("authorization".to_string(), "Bearer token-value".to_string()),
                ("x-project".to_string(), "token-value".to_string()),
                ("x-literal".to_string(), "$POSTHOG_PROJECT_TOKEN".to_string()),
            ])
        );
    }

    #[test]
    fn reports_missing_otlp_header_environment_variables() {
        let settings = OtlpTelemetrySettings {
            headers: BTreeMap::from([("authorization".to_string(), "Bearer $POSTHOG_PROJECT_TOKEN".to_string())]),
            ..OtlpTelemetrySettings::default()
        };

        assert!(matches!(
            settings.resolved_headers(&Vars::new().with_env_lookup(|_| None)),
            Err(VarError::NotFound(variable)) if variable == "POSTHOG_PROJECT_TOKEN"
        ));
    }

    #[test]
    fn parses_credentials_store_encrypted_file_with_options() {
        let config = SettingsJson::new()
            .field(
                "credentialsStore",
                json!({
                    "type": "encryptedFile",
                    "path": "/custom/creds.enc",
                    "passwordEnv": "MY_SECRET"
                }),
            )
            .agent(AgentJson::new("alpha", "Alpha"))
            .parse();

        assert!(matches!(
            &config.credentials_store,
            Some(CredentialsStoreConfig::EncryptedFile { path, password_env })
            if path == &Some(PathBuf::from("/custom/creds.enc"))
                && password_env == &Some("MY_SECRET".to_string())
        ));
    }
}
