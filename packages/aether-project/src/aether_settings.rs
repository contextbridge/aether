use utils::SettingsStore;

use crate::agent_config::AgentConfig;
use crate::error::SettingsError;
use crate::{McpSourceSpec, PromptSource};
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

const PROJECT_SETTINGS_PATH: &str = ".aether/settings.json";

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AetherSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<PromptSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<McpSourceSpec>,
    #[schemars(length(min = 1))]
    pub agents: Vec<AgentConfig>,
}

#[derive(Debug, Clone)]
pub enum AetherSettingsSource {
    File(PathBuf),
    OptionalFile(PathBuf),
    Json(String),
    Value(AetherSettings),
}

impl AetherSettings {
    pub fn load_default(project_root: &Path) -> Result<Self, SettingsError> {
        Self::load(project_root, default_sources())
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
        if next.agent.is_some() {
            self.agent = next.agent;
        }

        if !next.prompts.is_empty() {
            self.prompts = next.prompts;
        }
        if !next.mcps.is_empty() {
            self.mcps = next.mcps;
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

    fn load_source(project_root: &Path, source: AetherSettingsSource) -> Result<Self, SettingsError> {
        match source {
            AetherSettingsSource::File(path) => load_file(&source_path(project_root, path), false),
            AetherSettingsSource::OptionalFile(path) => load_file(&source_path(project_root, path), true),
            AetherSettingsSource::Json(json) => Self::try_from(json.as_str()),
            AetherSettingsSource::Value(config) => Ok(config),
        }
    }
}

fn default_sources() -> Vec<AetherSettingsSource> {
    let aether_home = SettingsStore::new("AETHER_HOME", ".aether").map(|store| store.home().to_path_buf());
    default_sources_for_home(aether_home.as_deref())
}

fn default_sources_for_home(aether_home: Option<&Path>) -> Vec<AetherSettingsSource> {
    let mut sources = Vec::new();
    if let Some(aether_home) = aether_home {
        sources.push(AetherSettingsSource::OptionalFile(aether_home.join("settings.json")));
    }
    sources.push(AetherSettingsSource::OptionalFile(PathBuf::from(PROJECT_SETTINGS_PATH)));
    sources
}

fn source_path(project_root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { project_root.join(path) }
}

fn load_file(path: &Path, missing_is_empty: bool) -> Result<AetherSettings, SettingsError> {
    match read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(AetherSettings::default()),
        Ok(content) => AetherSettings::try_from(content.as_str()),
        Err(error) if missing_is_empty && error.kind() == std::io::ErrorKind::NotFound => Ok(AetherSettings::default()),
        Err(error) => Err(SettingsError::IoError(format!("Failed to read {}: {}", path.display(), error))),
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
    use crate::{AgentCatalog, McpSourceSpec, PromptSource};
    use aether_core::agent_spec::McpConfigSource;
    use std::collections::BTreeMap;
    use std::fs::{create_dir_all, write};
    use std::path::PathBuf;

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
            r#"{"agents":[{"name":"alpha","description":"Alpha","model":"anthropic:claude-sonnet-4-5","userInvocable":true,"prompts":[{"type":"file","path":"PROMPT.md"}]}]}"#,
        );

        let config =
            AetherSettings::load(dir.path(), [AetherSettingsSource::File(PathBuf::from("nested/config.json"))])
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
            [AetherSettingsSource::Value(base), AetherSettingsSource::Value(override_config)],
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
            r#"{
                "agent":"shared",
                "prompts":["USER.md"],
                "agents":[
                    {"name":"shared","description":"User shared","model":"anthropic:claude-sonnet-4-5","userInvocable":true},
                    {"name":"user-only","description":"User only","model":"anthropic:claude-sonnet-4-5","userInvocable":true}
                ]
            }"#,
        );
        write_file(
            project.path(),
            ".aether/settings.json",
            r#"{
                "agent":"project-only",
                "prompts":["PROJECT.md"],
                "agents":[
                    {"name":"shared","description":"Project shared","model":"anthropic:claude-sonnet-4-5","userInvocable":true},
                    {"name":"project-only","description":"Project only","model":"anthropic:claude-sonnet-4-5","userInvocable":true}
                ]
            }"#,
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
            r#"{"agents":[{"name":"user-only","description":"User only","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]}"#,
        );

        let config = load_default_from_home(project.path(), &aether_home).unwrap();
        assert_eq!(
            config,
            AetherSettings { agents: vec![settings_agent("user-only", "User only")], ..AetherSettings::default() }
        );
    }

    #[test]
    fn load_default_uses_project_settings_when_user_settings_are_missing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(
            project.path(),
            ".aether/settings.json",
            r#"{"agents":[{"name":"project-only","description":"Project only","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]}"#,
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
        let err = AetherSettings::load(project.path(), [AetherSettingsSource::File(PathBuf::from("missing.json"))])
            .unwrap_err();

        assert!(matches!(err, SettingsError::IoError(_)));
    }

    #[test]
    fn optional_file_source_returns_default_when_missing() {
        let project = tempfile::tempdir().unwrap();
        let config =
            AetherSettings::load(project.path(), [AetherSettingsSource::OptionalFile(PathBuf::from("missing.json"))])
                .unwrap();

        assert_eq!(config, AetherSettings::default());
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
        let config = AetherSettings::try_from(
            r#"{
                "prompts": [{"type":"file","path":"BASE.md"}],
                "mcps": [{"type":"file","path":"mcp.json"}],
                "agents": [{
                    "name":"alpha",
                    "description":"Alpha",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true
                }]
            }"#,
        )
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
        let config = AetherSettings::try_from(
            r#"{
                "prompts": ["BASE.md"],
                "mcps": ["mcp.json"],
                "agents": [{
                    "name":"alpha",
                    "description":"Alpha",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true,
                    "prompts":["AGENT.md"],
                    "mcps":["agent-mcp.json"]
                }]
            }"#,
        )
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
    fn serializes_proxied_mcp_file_as_typed_object() {
        let source = McpSourceSpec::File { path: "mcp.json".to_string(), proxy: true };

        let value = serde_json::to_value(source).unwrap();

        assert_eq!(value, serde_json::json!({"type":"file", "path":"mcp.json", "proxy":true}));
    }

    #[test]
    fn rejects_old_top_level_mcp_servers_field() {
        let err = AetherSettings::try_from(
            r#"{
                "mcpServers": ["mcp.json"],
                "agents": [{
                    "name":"alpha",
                    "description":"Alpha",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true,
                    "prompts":[{"type":"file","path":"PROMPT.md"}]
                }]
            }"#,
        )
        .unwrap_err();

        assert!(matches!(err, SettingsError::ParseError(message) if message.contains("mcpServers")));
    }

    fn load_default_from_home(project_root: &Path, aether_home: &Path) -> Result<AetherSettings, SettingsError> {
        AetherSettings::load(project_root, default_sources_for_home(Some(aether_home)))
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
}
