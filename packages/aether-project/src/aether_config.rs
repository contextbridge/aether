use crate::agent_config::{AetherConfigSource, AgentConfig};
use crate::error::SettingsError;
use std::fs::read_to_string;
use std::path::Path;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AetherConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[schemars(length(min = 1))]
    pub agents: Vec<AgentConfig>,
}

impl AetherConfig {
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
        sources: impl IntoIterator<Item = AetherConfigSource>,
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

        for next_agent in next.agents {
            if let Some(existing) = self.agents.iter_mut().find(|agent| agent.name.trim() == next_agent.name.trim()) {
                *existing = next_agent;
            } else {
                self.agents.push(next_agent);
            }
        }

        self
    }

    fn load_source(project_root: &Path, source: AetherConfigSource) -> Result<Self, SettingsError> {
        match source {
            AetherConfigSource::File(path) => {
                let path = project_root.join(path);
                let content = read_to_string(&path)
                    .map_err(|e| SettingsError::IoError(format!("Failed to read {}: {}", path.display(), e)))?;
                Self::try_from(content.as_str())
            }
            AetherConfigSource::Json(json) => Self::try_from(json.as_str()),
            AetherConfigSource::Value(config) => Ok(config),
        }
    }
}

impl TryFrom<&str> for AetherConfig {
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
        let config =
            AetherConfig { agent: Some("beta".to_string()), agents: vec![agent_config("alpha"), agent_config("beta")] };

        let catalog = AgentCatalog::from_config(dir.path(), config).unwrap();

        assert_eq!(catalog.default_agent().map(|spec| spec.name.as_str()), Some("beta"));
    }

    #[test]
    fn rejects_selected_agent_that_is_not_user_invocable() {
        let mut internal = agent_config("internal");
        internal.user_invocable = false;
        internal.agent_invocable = true;
        let config = AetherConfig { agent: Some("internal".to_string()), agents: vec![internal] };

        let err = AgentCatalog::from_config(Path::new("/tmp"), config).unwrap_err();

        assert!(matches!(err, SettingsError::NonUserInvocableAgentSelector { .. }));
    }

    #[test]
    fn config_file_paths_are_project_relative() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        write_file(
            dir.path(),
            "nested/config.json",
            r#"{"agents":[{"name":"alpha","description":"Alpha","model":"anthropic:claude-sonnet-4-5","userInvocable":true,"prompts":[{"type":"file","path":"PROMPT.md"}]}]}"#,
        );

        let config =
            AetherConfig::load(dir.path(), [AetherConfigSource::File(PathBuf::from("nested/config.json"))]).unwrap();
        let catalog = AgentCatalog::from_config(dir.path(), config).unwrap();

        assert_eq!(catalog.all()[0].name, "alpha");
    }

    #[test]
    fn load_merges_sources_with_rightmost_agent_winning() {
        let dir = tempfile::tempdir().unwrap();
        let base = AetherConfig {
            agent: Some("alpha".to_string()),
            agents: vec![AgentConfig { description: "Base alpha".to_string(), ..agent_config("alpha") }],
        };
        let override_config = AetherConfig {
            agent: Some("beta".to_string()),
            agents: vec![
                AgentConfig { description: "Override alpha".to_string(), ..agent_config("alpha") },
                agent_config("beta"),
            ],
        };

        let config = AetherConfig::load(
            dir.path(),
            [AetherConfigSource::Value(base), AetherConfigSource::Value(override_config)],
        )
        .unwrap();

        assert_eq!(config.agent.as_deref(), Some("beta"));
        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents[0].name, "alpha");
        assert_eq!(config.agents[0].description, "Override alpha");
        assert_eq!(config.agents[1].name, "beta");
    }

    #[test]
    fn resolves_inline_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        let config = AetherConfig {
            agent: None,
            agents: vec![AgentConfig {
                mcps: vec![McpSourceSpec::Inline { servers: BTreeMap::new() }],
                ..agent_config("alpha")
            }],
        };

        let catalog = AgentCatalog::from_config(dir.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert_eq!(spec.mcp_config_sources.len(), 1);
        assert!(matches!(spec.mcp_config_sources[0], McpConfigSource::Inline(_)));
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
