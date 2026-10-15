use utils::settings::aether_home;

use crate::agent_config::AgentConfig;
use crate::error::SettingsError;
use crate::{McpSourceSpec, PromptSource};
use aether_core::core::Prompt;
use llm::ProviderConnectionOverrides;
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
        /// `credentials.enc` in the Aether home directory when unset.
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
    aether_home().map(|home| home.join(USER_SETTINGS_FILENAME))
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
    /// Per-attribute content capture, mapping 1:1 onto the opt-in `GenAI`
    /// content attributes. All default to `false`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub content: TelemetryContentSettings,
    /// Trace signal toggle. Enabled by default.
    #[serde(default, skip_serializing_if = "is_default")]
    pub traces: TelemetrySignalSettings,
    /// Metric signal toggle. Enabled by default.
    #[serde(default, skip_serializing_if = "is_default")]
    pub metrics: TelemetrySignalSettings,
    #[serde(default, skip_serializing_if = "is_default")]
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
pub struct TelemetryContentSettings {
    /// Set `gen_ai.system_instructions` on chat spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_instructions: Option<bool>,
    /// Set `gen_ai.input.messages` on turn and chat spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_messages: Option<bool>,
    /// Set `gen_ai.output.messages` on turn and chat spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_messages: Option<bool>,
    /// Set `gen_ai.tool.definitions` on chat spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_definitions: Option<bool>,
    /// Set `gen_ai.tool.call.arguments` / `gen_ai.tool.call.result` on tool spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<bool>,
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

    pub fn traces_enabled(&self) -> bool {
        self.traces.enabled.unwrap_or(true)
    }

    pub fn metrics_enabled(&self) -> bool {
        self.metrics.enabled.unwrap_or(true)
    }

    fn merge(&mut self, next: Self) {
        merge_field(&mut self.service_name, next.service_name);
        merge_field(&mut self.sample_ratio, next.sample_ratio);
        self.content.merge(&next.content);
        merge_field(&mut self.traces.enabled, next.traces.enabled);
        merge_field(&mut self.metrics.enabled, next.metrics.enabled);
        merge_field(&mut self.otlp.endpoint, next.otlp.endpoint);
        merge_field(&mut self.otlp.traces_endpoint, next.otlp.traces_endpoint);
        merge_field(&mut self.otlp.metrics_endpoint, next.otlp.metrics_endpoint);
        self.otlp.headers.extend(next.otlp.headers);
    }
}

impl TelemetryContentSettings {
    fn merge(&mut self, next: &Self) {
        merge_field(&mut self.system_instructions, next.system_instructions);
        merge_field(&mut self.input_messages, next.input_messages);
        merge_field(&mut self.output_messages, next.output_messages);
        merge_field(&mut self.tool_definitions, next.tool_definitions);
        merge_field(&mut self.tool_calls, next.tool_calls);
    }
}

impl OtlpTelemetrySettings {
    pub fn resolved_headers(&self, vars: &Vars) -> Result<BTreeMap<String, String>, VarError> {
        self.headers.iter().map(|(name, value)| Ok((name.clone(), vars.expand(value)?))).collect()
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
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

    pub fn merge(mut self, next: Self) -> Self {
        merge_field(&mut self.agent, next.agent);
        if !next.prompts.is_empty() {
            self.prompts = next.prompts;
        }
        if !next.mcps.is_empty() {
            self.mcps = next.mcps;
        }
        self.providers.merge(next.providers);

        merge_field(&mut self.credentials_store, next.credentials_store);
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
        for (prompts, mcps) in self.resource_sources_mut() {
            *prompts = inline_prompt_sources(prompts, root)?;
            *mcps = inline_mcp_sources(mcps, root)?;
        }
        Ok(())
    }

    fn resource_sources_mut(&mut self) -> impl Iterator<Item = (&mut Vec<PromptSource>, &mut Vec<McpSourceSpec>)> {
        std::iter::once((&mut self.prompts, &mut self.mcps))
            .chain(self.agents.iter_mut().map(|agent| (&mut agent.prompts, &mut agent.mcps)))
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
    default_sources_for_home(project_root, aether_home().as_deref())
}

fn default_sources_for_home(project_root: &Path, aether_home: Option<&Path>) -> Vec<AetherSettingsSource> {
    let mut sources = Vec::new();
    if let Some(aether_home) = aether_home {
        sources.push(AetherSettingsSource::OptionalFile(SettingsFileSource::new(USER_SETTINGS_FILENAME, aether_home)));
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
    for (prompts, mcps) in settings.resource_sources_mut() {
        promote_prompt_sources(prompts, root);
        promote_mcp_sources(mcps, root);
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
    sources
        .iter()
        .filter_map(|spec| spec.load(root).transpose())
        .map(|config| config.map(McpSourceSpec::Inline))
        .collect()
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
    use crate::testing::{
        agent_config, agent_json, agent_json_with, has_prompt_file, home, mcp_json, project, settings_agent,
        settings_with_mcps,
    };
    use crate::{AgentCatalog, McpFileSpec, McpSourceSpec, PromptSource};
    use aether_core::mcp::McpConfig;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn telemetry_is_disabled_when_absent() {
        assert!(AetherSettings::default().telemetry.is_none());

        let settings = AetherSettings::try_from(r#"{ "telemetry": {}, "agents": [] }"#).unwrap();
        assert!(settings.telemetry.unwrap().effective_enabled());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_telemetry_camel_case_and_http_protobuf() {
        let config = serde_json::from_value::<AetherSettings>(json!({
            "telemetry": {
                "serviceName": "aether-test",
                "sampleRatio": 0.5,
                "content": {
                    "systemInstructions": true,
                    "inputMessages": true,
                    "outputMessages": false,
                    "toolDefinitions": true,
                    "toolCalls": true
                },
                "traces": { "enabled": true },
                "metrics": { "enabled": false },
                "otlp": {
                    "endpoint": "http://localhost:4318",
                    "headers": { "authorization": "Bearer token" }
                }
            },
            "agents": [agent_json("alpha", "Alpha")]
        }))
        .unwrap();

        let telemetry = config.telemetry.as_ref().unwrap();
        assert_eq!(telemetry.service_name(), "aether-test");
        assert_eq!(telemetry.sample_ratio(), 0.5);
        let content = &telemetry.content;
        assert_eq!(content.system_instructions, Some(true));
        assert_eq!(content.input_messages, Some(true));
        assert_eq!(content.output_messages, Some(false));
        assert_eq!(content.tool_definitions, Some(true));
        assert_eq!(content.tool_calls, Some(true));
        assert!(telemetry.traces_enabled());
        assert!(!telemetry.metrics_enabled());
        assert_eq!(telemetry.otlp.headers.get("authorization").map(String::as_str), Some("Bearer token"));
    }

    #[test]
    fn telemetry_with_no_enabled_signals_does_not_require_endpoint() {
        let config = serde_json::from_value::<AetherSettings>(json!({
            "telemetry": { "traces": { "enabled": false }, "metrics": { "enabled": false } },
            "agents": [agent_json("alpha", "Alpha")]
        }))
        .unwrap();

        assert!(!config.telemetry.unwrap().effective_enabled());
    }

    #[test]
    fn telemetry_overlays_merge_fields_and_allow_explicit_defaults() {
        let config = AetherSettings::load(
            Path::new("/project"),
            [
                AetherSettingsSource::Json(
                    r#"{
                        "telemetry": {
                            "content": { "systemInstructions": true, "inputMessages": true },
                            "traces": { "enabled": true },
                            "metrics": { "enabled": true },
                            "otlp": {
                                "endpoint": "http://localhost:4318",
                                "tracesEndpoint": "https://traces.example.com/export",
                                "metricsEndpoint": "https://metrics.example.com/export"
                            }
                        },
                        "agents": []
                    }"#
                    .to_string(),
                ),
                AetherSettingsSource::Json(
                    r#"{
                        "telemetry": {
                            "content": { "systemInstructions": false },
                            "metrics": { "enabled": false }
                        },
                        "agents": []
                    }"#
                    .to_string(),
                ),
            ],
        )
        .unwrap();

        let telemetry = config.telemetry.unwrap();
        let content = &telemetry.content;
        assert_eq!(content.system_instructions, Some(false), "explicit false remains distinguishable from omission");
        assert_eq!(content.input_messages, Some(true), "omitted nested fields remain inherited");
        assert!(!content.tool_calls.unwrap_or_default());
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
                    r#"{
                        "telemetry": {
                            "otlp": {
                                "endpoint": "http://localhost:4318",
                                "headers": { "authorization": "Bearer base", "x-base": "1" }
                            }
                        },
                        "agents": []
                    }"#
                    .to_string(),
                ),
                AetherSettingsSource::Json(
                    r#"{
                        "telemetry": {
                            "otlp": {
                                "headers": { "authorization": "Bearer overlay", "x-overlay": "2" }
                            }
                        },
                        "agents": []
                    }"#
                    .to_string(),
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
                    r#"{
                        "telemetry": {
                            "sampleRatio": 0.25,
                            "otlp": { "endpoint": "http://localhost:4318" }
                        },
                        "agents": []
                    }"#
                    .to_string(),
                ),
                AetherSettingsSource::Json(
                    r#"{ "telemetry": { "content": { "toolCalls": true } }, "agents": [] }"#.to_string(),
                ),
            ],
        )
        .unwrap();

        let telemetry = config.telemetry.unwrap();
        assert_eq!(telemetry.content.tool_calls, Some(true));
        assert!((telemetry.sample_ratio() - 0.25).abs() < f64::EPSILON);
        assert_eq!(telemetry.otlp.endpoint.as_deref(), Some("http://localhost:4318"));
    }

    #[test]
    fn project_settings_path_points_at_project_aether_settings() {
        assert_eq!(project_settings_path(Path::new("/repo")), PathBuf::from("/repo/.aether/settings.json"));
    }

    #[test]
    fn project_settings_exist_checks_project_settings_file() {
        let project = project();
        assert!(!project_settings_exist(project.root()));
        project.write(PROJECT_SETTINGS_PATH, "{}");
        assert!(project_settings_exist(project.root()));
    }

    #[test]
    fn resolves_selected_agent() {
        let dir = project().file("PROMPT.md", "Be helpful");
        let config = AetherSettings {
            agent: Some("beta".to_string()),
            agents: vec![agent_config("alpha"), agent_config("beta")],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.root(), config).unwrap();

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
        let config_json =
            json!({ "agents": [agent_json_with("alpha", "Alpha", json!({ "prompts": ["PROMPT.md"] }))] }).to_string();
        let dir = project().file("PROMPT.md", "Be helpful").file("nested/config.json", &config_json);

        let config = AetherSettings::load(
            dir.root(),
            [AetherSettingsSource::File(SettingsFileSource::new("nested/config.json", dir.root()))],
        )
        .unwrap();
        let catalog = AgentCatalog::from_settings(dir.root(), config).unwrap();

        assert_eq!(catalog.all()[0].name, "alpha");
    }

    #[test]
    fn load_merges_sources_with_rightmost_agent_winning() {
        let dir = project();
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
            dir.root(),
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
        let project = project();
        let home = home().file(
            ".aether/settings.json",
            json!({
                "agent": "shared",
                "prompts": ["USER.md"],
                "agents": [agent_json("shared", "User shared"), agent_json("user-only", "User only")]
            })
            .to_string(),
        );
        project.write(
            ".aether/settings.json",
            json!({
                "agent": "project-only",
                "prompts": ["PROJECT.md"],
                "agents": [agent_json("shared", "Project shared"), agent_json("project-only", "Project only")]
            })
            .to_string(),
        );

        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();
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
        let project = project();
        let home = home()
            .file(".aether/settings.json", json!({ "agents": [agent_json("user-only", "User only")] }).to_string());

        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();
        assert_eq!(
            config,
            AetherSettings { agents: vec![settings_agent("user-only", "User only")], ..AetherSettings::default() }
        );
    }

    #[test]
    fn load_default_resolves_user_agent_paths_from_aether_home() {
        let project = project();
        let home = home()
            .file(".aether/agents/user.md", "User instructions")
            .file(".aether/mcp/user.json", mcp_json("user"))
            .file(
                ".aether/settings.json",
                json!({
                    "agents": [agent_json_with(
                        "user-only",
                        "User only",
                        json!({ "prompts": ["agents/user.md"], "mcps": ["mcp/user.json"] }),
                    )]
                })
                .to_string(),
            );

        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();
        let catalog = AgentCatalog::from_settings(project.root(), config).unwrap();
        let spec = catalog.resolve("user-only").unwrap();

        assert!(has_prompt_file(&spec, aether_home.join("agents/user.md")));
        assert_eq!(spec.mcp.servers.keys().collect::<Vec<_>>(), ["user"]);
    }

    #[test]
    fn load_default_uses_project_settings_when_user_settings_are_missing() {
        let home = home();
        let project = project().file(
            ".aether/settings.json",
            json!({ "agents": [agent_json("project-only", "Project only")] }).to_string(),
        );

        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();

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
        let project = project();
        let home = home();
        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();
        assert_eq!(config, AetherSettings::default());
    }

    #[test]
    fn load_default_rejects_malformed_user_settings() {
        let project = project();
        let home = home().file(".aether/settings.json", "{not-json");
        let aether_home = home.path(".aether");
        let err = load_default_from_home(project.root(), &aether_home).unwrap_err();
        assert!(matches!(err, SettingsError::ParseError(_)));
    }

    #[test]
    fn strict_file_source_errors_when_missing() {
        let project = project();
        let err = AetherSettings::load(
            project.root(),
            [AetherSettingsSource::File(SettingsFileSource::new("missing.json", project.root()))],
        )
        .unwrap_err();

        assert!(matches!(err, SettingsError::IoError(_)));
    }

    #[test]
    fn optional_file_source_returns_default_when_missing() {
        let project = project();
        let config = AetherSettings::load(
            project.root(),
            [AetherSettingsSource::OptionalFile(SettingsFileSource::new("missing.json", project.root()))],
        )
        .unwrap();

        assert_eq!(config, AetherSettings::default());
    }

    #[test]
    fn inline_resources_replaces_file_sources_with_their_contents() {
        let project = project()
            .file("BASE.md", "Be helpful")
            .file("AGENT.md", "Edit carefully")
            .file("mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"run"}}}"#);

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

        settings.inline_resources(project.root()).unwrap();

        assert_eq!(settings.prompts, vec![PromptSource::Text { text: "Be helpful".to_string() }]);
        assert_eq!(settings.agents[0].prompts, vec![PromptSource::Text { text: "Edit carefully".to_string() }]);
        assert!(matches!(&settings.mcps[0], McpSourceSpec::Inline(config) if config.servers.contains_key("coding")));
        assert!(
            matches!(&settings.agents[0].mcps[0], McpSourceSpec::Inline(config) if config.servers.contains_key("coding"))
        );

        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("BASE.md") && !serialized.contains("mcp.json"), "{serialized}");
    }

    #[test]
    fn inline_resources_drops_optional_missing_sources() {
        let project = project().file("PROMPT.md", "Agent prompt");
        let mut settings = AetherSettings {
            prompts: vec![PromptSource::file("absent.md").optional()],
            mcps: vec![McpSourceSpec::File(McpFileSpec::new("absent.json").optional())],
            agents: vec![agent_config("alpha")],
            ..AetherSettings::default()
        };

        settings.inline_resources(project.root()).unwrap();

        assert_eq!(settings.prompts.len(), 0);
        assert_eq!(settings.mcps.len(), 0);
    }

    #[test]
    fn inline_resources_errors_on_required_missing_mcp() {
        let project = project();
        let mut settings = AetherSettings {
            mcps: vec![McpSourceSpec::file("absent.json")],
            agents: vec![agent_config("alpha")],
            ..AetherSettings::default()
        };

        let err = settings.inline_resources(project.root()).unwrap_err();
        assert!(matches!(err, SettingsError::InvalidMcpConfigPath { .. }));
    }

    #[test]
    fn parses_top_level_prompt_and_mcp_defaults() {
        let config = serde_json::from_value::<AetherSettings>(json!({
            "prompts": [{ "type": "file", "path": "BASE.md" }],
            "mcps": [{ "type": "file", "path": "mcp.json" }],
            "agents": [agent_json("alpha", "Alpha")]
        }))
        .unwrap();

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
        let config = serde_json::from_value::<AetherSettings>(json!({
            "prompts": ["BASE.md"],
            "mcps": ["mcp.json"],
            "agents": [agent_json_with(
                "alpha",
                "Alpha",
                json!({ "prompts": ["AGENT.md"], "mcps": ["agent-mcp.json"] }),
            )]
        }))
        .unwrap();

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
    fn serializes_deferred_mcp_file_as_typed_object() {
        let source: McpSourceSpec = McpFileSpec::new("mcp.json").defer_tools().into();

        let value = serde_json::to_value(source).unwrap();

        assert_eq!(value, serde_json::json!({"type":"file", "path":"mcp.json", "deferTools":true}));
    }

    #[test]
    fn rejects_old_top_level_mcp_servers_field() {
        let err = AetherSettings::try_from(
            json!({
                "mcpServers": ["mcp.json"],
                "agents": [agent_json_with("alpha", "Alpha", json!({ "prompts": ["PROMPT.md"] }))]
            })
            .to_string()
            .as_str(),
        )
        .unwrap_err();

        assert!(matches!(err, SettingsError::ParseError(message) if message.contains("mcpServers")));
    }

    #[test]
    fn load_default_resolves_workspace_scoped_user_prompt_and_mcp_paths() {
        let project = project().file("AGENTS.md", "Agent instructions").file(".aether/mcp.json", mcp_json("project"));
        let home = home().file(".aether/agents/planner/SYSTEM.md", "System instructions").file(
            ".aether/settings.json",
            json!({
                "agents": [agent_json_with(
                    "planner",
                    "Plans work",
                    json!({
                        "prompts": [
                            "agents/planner/SYSTEM.md",
                            { "type": "file", "path": "${WORKSPACE}/AGENTS.md" }
                        ],
                        "mcps": [{ "type": "file", "path": "${WORKSPACE}/.aether/mcp.json" }]
                    }),
                )]
            })
            .to_string(),
        );

        let aether_home = home.path(".aether");
        let config = load_default_from_home(project.root(), &aether_home).unwrap();
        let catalog = AgentCatalog::from_settings(project.root(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert!(has_prompt_file(&spec, aether_home.join("agents/planner/SYSTEM.md")));
        assert!(has_prompt_file(&spec, project.root().join("AGENTS.md")));
        assert_eq!(spec.mcp.servers.keys().collect::<Vec<_>>(), ["project"]);
    }

    #[test]
    fn workspace_scoped_paths_expand_in_project_settings_without_absolutizing_normal_relative_paths() {
        let project = project().file("PROJECT.md", "Project prompt").file("AGENTS.md", "Agent prompt").file(
            ".aether/settings.json",
            json!({
                "agents": [agent_json_with(
                    "alpha",
                    "Alpha",
                    json!({ "prompts": ["PROJECT.md", { "type": "file", "path": "${WORKSPACE}/AGENTS.md" }] }),
                )]
            })
            .to_string(),
        );

        let config = AetherSettings::load(
            project.root(),
            [AetherSettingsSource::OptionalFile(SettingsFileSource::new(PROJECT_SETTINGS_PATH, project.root()))],
        )
        .unwrap();

        assert_eq!(config.agents[0].prompts[0], PromptSource::file("PROJECT.md"));
        assert_eq!(config.agents[0].prompts[1], PromptSource::file("${WORKSPACE}/AGENTS.md"));
    }

    #[test]
    fn json_and_value_sources_preserve_workspace_scoped_paths_losslessly() {
        let project = project();

        let json_config = AetherSettings::load(
            project.root(),
            [AetherSettingsSource::Json(
                json!({
                    "agents": [agent_json_with("alpha", "Alpha", json!({ "prompts": ["${WORKSPACE}/AGENTS.md"] }))]
                })
                .to_string(),
            )],
        )
        .unwrap();

        assert_eq!(json_config.agents[0].prompts[0], PromptSource::file("${WORKSPACE}/AGENTS.md"));

        let value_config = AetherSettings::load(
            project.root(),
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
    fn mcp_sources_without_loadable_servers_resolve_to_no_servers() {
        let project = project().file("PROMPT.md", "Be helpful");
        let sources: [McpSourceSpec; 3] = [
            McpSourceSpec::Inline(McpConfig::default()),
            McpFileSpec::new("${WORKSPACE}/.aether/mcp.json").optional().into(),
            McpFileSpec::new("${DEFINITELY_NOT_SET_VAR_MCP_OPTIONAL}/mcp.json").optional().into(),
        ];

        for source in sources {
            let catalog =
                AgentCatalog::from_settings(project.root(), settings_with_mcps(vec![source.clone()])).unwrap();
            assert!(catalog.resolve("alpha").unwrap().mcp.is_empty(), "{source:?}");
        }
    }

    #[test]
    fn required_mcp_sources_that_cannot_load_error() {
        let project = project().file("PROMPT.md", "Be helpful");
        let error = |source| AgentCatalog::from_settings(project.root(), settings_with_mcps(vec![source])).unwrap_err();

        assert!(matches!(
            error(McpSourceSpec::file("${DEFINITELY_NOT_SET_VAR_MCP_REQ}/mcp.json")),
            SettingsError::UnresolvedMcpConfigVariable { .. }
        ));
        assert!(matches!(error(McpSourceSpec::file("nonexistent.json")), SettingsError::InvalidMcpConfigPath { .. }));
    }

    #[test]
    fn optional_existing_mcp_source_preserves_defer_tools_flag() {
        let project = project().file("PROMPT.md", "Be helpful").file("mcp.json", mcp_json("lazy"));
        let settings = settings_with_mcps(vec![McpFileSpec::new("mcp.json").defer_tools().optional().into()]);

        let spec = AgentCatalog::from_settings(project.root(), settings).unwrap().resolve("alpha").unwrap();

        assert!(spec.mcp.servers["lazy"].defer_tools().defers_tools());
    }

    #[test]
    fn optional_mcp_source_serializes_as_typed_object() {
        let source: McpSourceSpec = McpFileSpec::new("${WORKSPACE}/.aether/mcp.json").optional().into();
        let value = serde_json::to_value(source).unwrap();
        assert_eq!(value, serde_json::json!({"type":"file", "path":"${WORKSPACE}/.aether/mcp.json", "optional":true}));
    }

    #[test]
    fn all_optional_prompts_missing_errors_with_no_prompts() {
        let project = project();
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("MISSING.md").optional()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(project.root(), config).unwrap_err();
        assert!(matches!(err, SettingsError::AllOptionalPromptsMissing { agent } if agent == "alpha"));
    }

    #[test]
    fn settings_round_trip_preserves_workspace_prefix_and_relative_paths() {
        let original = json!({
            "agents": [agent_json_with(
                "alpha",
                "Alpha",
                json!({
                    "prompts": [
                        "AGENTS.md",
                        "${WORKSPACE}/SYSTEM.md",
                        { "type": "file", "path": "${WORKSPACE}/.aether/rules.md", "optional": true },
                        { "type": "glob", "pattern": "${WORKSPACE}/.aether/rules/*.md" }
                    ],
                    "mcps": [
                        "mcp.json",
                        { "type": "file", "path": "${WORKSPACE}/.aether/mcp.json", "optional": true }
                    ]
                }),
            )]
        });

        let settings: AetherSettings = serde_json::from_value(original).unwrap();
        let reparsed: AetherSettings = serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();

        assert_eq!(settings, reparsed, "settings should round-trip losslessly through serde");
    }

    #[test]
    fn user_settings_relative_paths_absolutize_at_load_but_workspace_token_is_preserved() {
        let project = project().file("AGENTS.md", "agents");
        let home = home().file(".aether/agents/planner/SYSTEM.md", "system").file(
            ".aether/settings.json",
            json!({
                "agents": [agent_json_with(
                    "planner",
                    "Plans",
                    json!({ "prompts": ["agents/planner/SYSTEM.md", "${WORKSPACE}/AGENTS.md"] }),
                )]
            })
            .to_string(),
        );

        let aether_home = home.path(".aether");
        let settings = load_default_from_home(project.root(), &aether_home).unwrap();

        let expected_user = aether_home.join("agents/planner/SYSTEM.md").to_string_lossy().to_string();
        assert_eq!(
            settings.agents[0].prompts,
            vec![PromptSource::file(expected_user), PromptSource::file("${WORKSPACE}/AGENTS.md")],
            "user-rooted relative paths must absolutize; ${{WORKSPACE}}/ paths must be preserved",
        );
    }

    fn load_default_from_home(project_root: &Path, aether_home: &Path) -> Result<AetherSettings, SettingsError> {
        AetherSettings::load(project_root, default_sources_for_home(project_root, Some(aether_home)))
    }

    #[test]
    fn parses_credentials_store_variants() {
        let encrypted_file = CredentialsStoreConfig::EncryptedFile {
            path: Some(PathBuf::from("/custom/creds.enc")),
            password_env: Some("MY_SECRET".to_string()),
        };
        let cases = [
            (json!({ "type": "keyring" }), CredentialsStoreConfig::Keyring),
            (json!({ "type": "memory" }), CredentialsStoreConfig::Memory),
            (
                json!({ "type": "encryptedFile", "path": "/custom/creds.enc", "passwordEnv": "MY_SECRET" }),
                encrypted_file,
            ),
        ];

        for (store, expected) in cases {
            let config: AetherSettings =
                serde_json::from_value(json!({ "credentialsStore": store, "agents": [] })).unwrap();
            assert_eq!(config.credentials_store, Some(expected));
        }
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
}
