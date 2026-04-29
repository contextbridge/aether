use crate::agent_config::AgentConfig;
use crate::error::SettingsError;
use crate::{McpSourceSpec, PromptSource};
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
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
    Json(String),
    Value(AetherSettings),
}

impl AetherSettings {
    pub fn load_default(project_root: &Path) -> Result<Self, SettingsError> {
        let settings_path = project_root.join(".aether/settings.json");
        match read_to_string(&settings_path) {
            Ok(content) if content.trim().is_empty() => Ok(Self::default()),
            Ok(content) => Self::try_from(content.as_str()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::IoError(format!("Failed to read {}: {}", settings_path.display(), e))),
        }
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
            AetherSettingsSource::File(path) => {
                let path = project_root.join(path);
                let content = read_to_string(&path)
                    .map_err(|e| SettingsError::IoError(format!("Failed to read {}: {}", path.display(), e)))?;
                Self::try_from(content.as_str())
            }
            AetherSettingsSource::Json(json) => Self::try_from(json.as_str()),
            AetherSettingsSource::Value(config) => Ok(config),
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

        assert_eq!(config.agent.as_deref(), Some("beta"));
        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents[0].name, "alpha");
        assert_eq!(config.agents[0].description, "Override alpha");
        assert_eq!(config.agents[1].name, "beta");
        assert_eq!(config.prompts, vec![PromptSource::file("OVERRIDE.md")]);
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

        assert_eq!(config.prompts, vec![PromptSource::file("BASE.md")]);
        assert_eq!(config.mcps[0].path(), Some("mcp.json"));
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

    fn write_file(dir: &Path, path: &str, content: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            create_dir_all(parent).unwrap();
        }

        write(full, content).unwrap();
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
