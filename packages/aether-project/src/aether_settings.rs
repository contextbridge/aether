use utils::SettingsStore;

use crate::agent_config::AgentConfig;
use crate::error::SettingsError;
use crate::{McpSourceSpec, PromptSource};
use llm::ProviderConnectionOverrides;
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
    #[serde(default, skip_serializing_if = "ProviderConnectionOverrides::is_empty")]
    pub providers: ProviderConnectionOverrides,
    #[schemars(length(min = 1))]
    pub agents: Vec<AgentConfig>,
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
    Value(AetherSettings),
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
            AetherSettingsSource::File(source) => load_file_source(project_root, source, false),
            AetherSettingsSource::OptionalFile(source) => load_file_source(project_root, source, true),
            AetherSettingsSource::Json(json) => Self::try_from(json.as_str()),
            AetherSettingsSource::Value(settings) => Ok(settings),
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
    use std::collections::BTreeMap;
    use std::fs::{create_dir_all, write};

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
    fn load_default_resolves_user_agent_paths_from_aether_home() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let aether_home = home.path().join(".aether");
        write_file(&aether_home, "agents/user.md", "User instructions");
        write_file(&aether_home, "mcp/user.json", r#"{"servers":{}}"#);
        write_file(
            &aether_home,
            "settings.json",
            r#"{
                "agents":[{
                    "name":"user-only",
                    "description":"User only",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true,
                    "prompts":["agents/user.md"],
                    "mcps":["mcp/user.json"]
                }]
            }"#,
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
            McpConfigSource::File { path, proxy: false } if path == &aether_home.join("mcp/user.json")
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
        let source: McpSourceSpec = McpFileSpec::new("mcp.json").proxy().into();

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
            r#"{
                "agents":[{
                    "name":"planner",
                    "description":"Plans work",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true,
                    "prompts":[
                        "agents/planner/SYSTEM.md",
                        {"type":"file","path":"${WORKSPACE}/AGENTS.md"}
                    ],
                    "mcps":[
                        {"type":"file","path":"${WORKSPACE}/.aether/mcp.json"}
                    ]
                }]
            }"#,
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
            McpConfigSource::File { path, proxy: false } if *path == project.path().join(".aether/mcp.json")
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
            r#"{
                "agents":[{
                    "name":"alpha",
                    "description":"Alpha",
                    "model":"anthropic:claude-sonnet-4-5",
                    "userInvocable":true,
                    "prompts":["PROJECT.md", {"type":"file","path":"${WORKSPACE}/AGENTS.md"}]
                }]
            }"#,
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
                r#"{
                    "agents":[{
                        "name":"alpha",
                        "description":"Alpha",
                        "model":"anthropic:claude-sonnet-4-5",
                        "userInvocable":true,
                        "prompts":["${WORKSPACE}/AGENTS.md"]
                    }]
                }"#
                .to_string(),
            )],
        )
        .unwrap();

        assert_eq!(json_config.agents[0].prompts[0], PromptSource::file("${WORKSPACE}/AGENTS.md"));

        let value_config = AetherSettings::load(
            project.path(),
            [AetherSettingsSource::Value(AetherSettings {
                agents: vec![AgentConfig {
                    prompts: vec![PromptSource::file("${WORKSPACE}/AGENTS.md")],
                    ..agent_config("alpha")
                }],
                ..AetherSettings::default()
            })],
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

        let config = AetherSettings::load(project.path(), [AetherSettingsSource::Value(config)]).unwrap();
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

        let config = AetherSettings::load(project.path(), [AetherSettingsSource::Value(config)]).unwrap();
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
    fn optional_existing_mcp_source_preserves_proxy_flag() {
        let project = tempfile::tempdir().unwrap();
        write_file(project.path(), "BASE.md", "Base instructions");
        write_file(project.path(), "mcp.json", r#"{"servers":{}}"#);
        let config = AetherSettings {
            agents: vec![AgentConfig {
                prompts: vec![PromptSource::file("BASE.md")],
                mcps: vec![McpFileSpec::new("mcp.json").proxy().optional().into()],
                ..agent_config("alpha")
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(project.path(), config).unwrap();
        let spec = catalog.resolve("alpha").unwrap();

        assert!(matches!(&spec.mcp_config_sources[0], McpConfigSource::File { proxy: true, .. }));
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
        let original = r#"{"agents":[{
            "name":"alpha",
            "description":"Alpha",
            "model":"anthropic:claude-sonnet-4-5",
            "userInvocable":true,
            "prompts":[
                "AGENTS.md",
                "${WORKSPACE}/SYSTEM.md",
                {"type":"file","path":"${WORKSPACE}/.aether/rules.md","optional":true},
                {"type":"glob","pattern":"${WORKSPACE}/.aether/rules/*.md"}
            ],
            "mcps":[
                "mcp.json",
                {"type":"file","path":"${WORKSPACE}/.aether/mcp.json","optional":true}
            ]
        }]}"#;

        let settings = AetherSettings::try_from(original).unwrap();
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
            r#"{"agents":[{
                "name":"planner",
                "description":"Plans",
                "model":"anthropic:claude-sonnet-4-5",
                "userInvocable":true,
                "prompts":["agents/planner/SYSTEM.md", "${WORKSPACE}/AGENTS.md"]
            }]}"#,
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
        AetherSettings::load(project_root, default_sources_for_home(project_root, Some(aether_home)))
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
