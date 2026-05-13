use crate::error::SettingsError;
use crate::{AetherSettings, AgentConfig, McpSourceSpec};
use aether_core::agent_spec::{AgentSpec, AgentSpecExposure, McpConfigSource};
use aether_core::core::Prompt;
use llm::{LlmModel, ProviderConnectionOverrides};
use mcp_utils::client::McpConfig;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A resolved catalog of agents from a project.
///
/// This type owns project-relative resolution context.
#[derive(Debug, Clone)]
pub struct AgentCatalog {
    project_root: PathBuf,
    specs: Vec<AgentSpec>,
    selected_agent: Option<String>,
}

impl AgentCatalog {
    pub fn from_settings(project_root: &Path, settings: AetherSettings) -> Result<Self, SettingsError> {
        validate_selected_agent(&settings)?;
        let selected_agent =
            settings.agent.as_deref().map(str::trim).filter(|name| !name.is_empty()).map(str::to_string);
        let defaults = AgentDefaults { prompts: settings.prompts, mcps: settings.mcps, providers: settings.providers };
        let mut seen_names = HashSet::new();
        let mut specs = Vec::with_capacity(settings.agents.len());
        for (index, entry) in settings.agents.into_iter().enumerate() {
            specs.push(resolve_agent_entry(project_root, entry, &defaults, index, &mut seen_names)?);
        }

        Ok(Self::new(project_root.to_path_buf(), specs, selected_agent))
    }

    pub(crate) fn new(project_root: PathBuf, specs: Vec<AgentSpec>, selected_agent: Option<String>) -> Self {
        Self { project_root, specs, selected_agent }
    }

    /// Create an empty catalog for a project with no settings.
    pub fn empty(project_root: PathBuf) -> Self {
        Self::new(project_root, Vec::new(), None)
    }

    /// The project root directory.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Get all agent specs in the catalog.
    pub fn all(&self) -> &[AgentSpec] {
        &self.specs
    }

    pub fn selected_agent(&self) -> Option<&str> {
        self.selected_agent.as_deref()
    }

    pub fn default_agent(&self) -> Option<&AgentSpec> {
        self.selected_agent
            .as_deref()
            .and_then(|name| self.specs.iter().find(|spec| spec.name == name))
            .or_else(|| self.user_invocable().next())
    }

    /// Get a specific agent by name.
    pub fn get(&self, name: &str) -> Result<&AgentSpec, SettingsError> {
        self.specs
            .iter()
            .find(|spec| spec.name == name)
            .ok_or_else(|| SettingsError::AgentNotFound { name: name.to_string() })
    }

    /// Iterate over user-invocable agents.
    pub fn user_invocable(&self) -> impl Iterator<Item = &AgentSpec> {
        self.specs.iter().filter(|s| s.exposure.user_invocable)
    }

    /// Iterate over agent-invocable agents.
    pub fn agent_invocable(&self) -> impl Iterator<Item = &AgentSpec> {
        self.specs.iter().filter(|s| s.exposure.agent_invocable)
    }

    /// Resolve and return a named agent spec ready for runtime use.
    pub fn resolve(&self, name: &str) -> Result<AgentSpec, SettingsError> {
        self.get(name).cloned()
    }
}

struct AgentDefaults {
    prompts: Vec<crate::PromptSource>,
    mcps: Vec<McpSourceSpec>,
    providers: ProviderConnectionOverrides,
}

fn validate_selected_agent(settings: &AetherSettings) -> Result<(), SettingsError> {
    if settings.agents.is_empty() {
        return Err(SettingsError::EmptyAgents);
    }

    if let Some(agent) = settings.agent.as_deref() {
        let selector = agent.trim();
        let Some(entry) = settings.agents.iter().find(|entry| entry.name.trim() == selector) else {
            return Err(SettingsError::InvalidAgentSelector { name: selector.to_string() });
        };

        if !entry.user_invocable {
            return Err(SettingsError::NonUserInvocableAgentSelector { name: selector.to_string() });
        }
    }

    Ok(())
}

fn resolve_agent_entry(
    project_root: &Path,
    entry: AgentConfig,
    defaults: &AgentDefaults,
    index: usize,
    seen_names: &mut HashSet<String>,
) -> Result<AgentSpec, SettingsError> {
    let name = entry.name.trim().to_string();
    if name.is_empty() {
        return Err(SettingsError::EmptyAgentName { index });
    }
    if name == "__default__" {
        return Err(SettingsError::ReservedAgentName { name });
    }
    if !seen_names.insert(name.clone()) {
        return Err(SettingsError::DuplicateAgentName { name });
    }

    let description = entry.description.trim().to_string();
    if description.is_empty() {
        return Err(SettingsError::MissingField { agent: name.clone(), field: "description".to_string() });
    }

    let model = parse_model(&name, &entry.model)?;
    if entry.context_window == Some(0) {
        return Err(SettingsError::InvalidContextWindow { agent: name.clone(), context_window: 0 });
    }
    if !entry.user_invocable && !entry.agent_invocable {
        return Err(SettingsError::NoInvocationSurface { agent: name.clone() });
    }
    let prompt_sources = if entry.prompts.is_empty() { &defaults.prompts } else { &entry.prompts };
    if prompt_sources.is_empty() {
        return Err(SettingsError::NoPrompts { agent: name.clone() });
    }

    let prompts = Prompt::from_sources(project_root, prompt_sources)
        .map_err(|source| SettingsError::AgentPromptSource { agent: name.clone(), source })?;
    let mcp_sources = if entry.mcps.is_empty() { &defaults.mcps } else { &entry.mcps };
    let mcp_config_sources = resolve_mcp_config_sources(project_root, mcp_sources)?;
    let mut provider_connections = defaults.providers.clone();
    provider_connections.merge(entry.providers);

    Ok(AgentSpec {
        name,
        description,
        model,
        reasoning_effort: entry.reasoning_effort,
        context_window: entry.context_window,
        prompts,
        provider_connections,
        mcp_config_sources,
        exposure: AgentSpecExposure { user_invocable: entry.user_invocable, agent_invocable: entry.agent_invocable },
        tools: entry.tools,
    })
}

fn resolve_mcp_config_sources(
    project_root: &Path,
    entries: &[McpSourceSpec],
) -> Result<Vec<McpConfigSource>, SettingsError> {
    entries
        .iter()
        .map(|entry| match entry {
            McpSourceSpec::File { path, proxy } => {
                let full_path = project_root.join(path);
                if full_path.is_file() {
                    Ok(McpConfigSource::file(full_path, *proxy))
                } else {
                    Err(SettingsError::InvalidMcpConfigPath { path: path.clone() })
                }
            }
            McpSourceSpec::Inline { servers } => Ok(McpConfigSource::Inline(McpConfig { servers: servers.clone() })),
        })
        .collect()
}

fn parse_model(agent: &str, model: &str) -> Result<String, SettingsError> {
    canonicalize_model_spec(model).map_err(|error| SettingsError::InvalidModel {
        agent: agent.to_string(),
        model: model.to_string(),
        error,
    })
}

fn canonicalize_model_spec(model: &str) -> Result<String, String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err("Model spec cannot be empty".to_string());
    }

    let mut canonical_parts = Vec::new();
    for part in trimmed.split(',').map(str::trim) {
        if part.is_empty() {
            return Err("Model spec contains an empty entry".to_string());
        }
        part.parse::<LlmModel>().map_err(|error: String| error)?;
        canonical_parts.push(part.to_string());
    }

    Ok(canonical_parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::agent_spec::{AgentSpecExposure, ToolFilter};
    use std::fs;

    fn create_temp_project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(dir: &Path, path: &str, content: &str) {
        let full_path = dir.join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full_path, content).unwrap();
    }

    fn make_spec(name: &str, exposure: AgentSpecExposure) -> AgentSpec {
        AgentSpec {
            name: name.to_string(),
            description: format!("{name} agent"),
            model: "anthropic:claude-sonnet-4-5".to_string(),
            reasoning_effort: None,
            context_window: None,
            prompts: vec![],
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure,
            tools: ToolFilter::default(),
        }
    }

    fn create_test_catalog(project_root: PathBuf) -> AgentCatalog {
        let planner = make_spec("planner", AgentSpecExposure::both());
        AgentCatalog::new(project_root, vec![planner], None)
    }

    fn file_sources(spec: &AgentSpec) -> Vec<(PathBuf, bool)> {
        spec.mcp_config_sources
            .iter()
            .filter_map(|source| match source {
                McpConfigSource::File { path, proxy } => Some((path.clone(), *proxy)),
                McpConfigSource::Json(_) | McpConfigSource::Inline(_) => None,
            })
            .collect()
    }

    fn has_prompt_file(spec: &AgentSpec, expected: &str) -> bool {
        spec.prompts.iter().any(|prompt| match prompt {
            Prompt::File { path, .. } => path == expected,
            Prompt::Text(_) | Prompt::PromptGlobs { .. } | Prompt::McpInstructions(_) => false,
        })
    }

    #[test]
    fn user_invocable_filters_correctly() {
        let dir = create_temp_project();
        let root = dir.path().to_path_buf();
        let catalog = AgentCatalog::new(
            root,
            vec![
                make_spec("planner", AgentSpecExposure::both()),
                make_spec("internal", AgentSpecExposure::agent_only()),
            ],
            None,
        );

        let user_invocable: Vec<_> = catalog.user_invocable().collect();
        assert_eq!(user_invocable.len(), 1);
        assert_eq!(user_invocable[0].name, "planner");
    }

    #[test]
    fn agent_invocable_filters_correctly() {
        let dir = create_temp_project();
        let root = dir.path().to_path_buf();
        let catalog = AgentCatalog::new(
            root,
            vec![
                make_spec("planner", AgentSpecExposure::both()),
                make_spec("user-only", AgentSpecExposure::user_only()),
            ],
            None,
        );

        let agent_invocable: Vec<_> = catalog.agent_invocable().collect();
        assert_eq!(agent_invocable.len(), 1);
        assert_eq!(agent_invocable[0].name, "planner");
    }

    #[test]
    fn default_agent_uses_selected_agent() {
        let dir = create_temp_project();
        let catalog = AgentCatalog::new(
            dir.path().to_path_buf(),
            vec![make_spec("first", AgentSpecExposure::both()), make_spec("second", AgentSpecExposure::both())],
            Some("second".to_string()),
        );

        assert_eq!(catalog.default_agent().map(|spec| spec.name.as_str()), Some("second"));
    }

    #[test]
    fn default_agent_falls_back_to_first_user_invocable() {
        let dir = create_temp_project();
        let catalog = AgentCatalog::new(
            dir.path().to_path_buf(),
            vec![
                make_spec("internal", AgentSpecExposure::agent_only()),
                make_spec("visible", AgentSpecExposure::user_only()),
            ],
            None,
        );

        assert_eq!(catalog.default_agent().map(|spec| spec.name.as_str()), Some("visible"));
    }

    #[test]
    fn get_returns_error_for_missing_agent() {
        let dir = create_temp_project();
        let catalog = create_test_catalog(dir.path().to_path_buf());
        let result = catalog.get("nonexistent");
        assert!(matches!(result, Err(SettingsError::AgentNotFound { .. })));
    }

    #[test]
    fn agent_context_window_is_resolved_into_spec() {
        let dir = create_temp_project();
        write_file(dir.path(), "BASE.md", "Base instructions");

        let config = AetherSettings {
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                context_window: Some(200_000),
                user_invocable: true,
                prompts: vec![crate::PromptSource::file("BASE.md")],
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert_eq!(spec.context_window, Some(200_000));
    }

    #[test]
    fn agent_context_window_rejects_zero() {
        let config = AetherSettings {
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                context_window: Some(0),
                user_invocable: true,
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(Path::new("/tmp"), config).unwrap_err();

        assert!(matches!(
            err,
            SettingsError::InvalidContextWindow { agent, context_window: 0 } if agent == "planner"
        ));
    }

    #[test]
    fn top_level_prompts_are_inherited_when_agent_prompts_are_empty() {
        let dir = create_temp_project();
        write_file(dir.path(), "BASE.md", "Base instructions");

        let config = AetherSettings {
            prompts: vec![crate::PromptSource::file("BASE.md")],
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                user_invocable: true,
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert!(has_prompt_file(&spec, "BASE.md"));
    }

    #[test]
    fn agent_prompts_override_top_level_prompts() {
        let dir = create_temp_project();
        write_file(dir.path(), "BASE.md", "Base instructions");
        write_file(dir.path(), "AGENT.md", "Agent instructions");

        let config = AetherSettings {
            prompts: vec![crate::PromptSource::file("BASE.md")],
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                user_invocable: true,
                prompts: vec![crate::PromptSource::file("AGENT.md")],
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert!(has_prompt_file(&spec, "AGENT.md"));
        assert!(!has_prompt_file(&spec, "BASE.md"));
    }

    #[test]
    fn top_level_mcps_are_inherited_when_agent_mcps_are_empty() {
        let dir = create_temp_project();
        write_file(dir.path(), "BASE.md", "Base instructions");
        write_file(dir.path(), "base-mcp.json", "{}");

        let config = AetherSettings {
            prompts: vec![crate::PromptSource::file("BASE.md")],
            mcps: vec![McpSourceSpec::file("base-mcp.json")],
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                user_invocable: true,
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert_eq!(file_sources(&spec), vec![(dir.path().join("base-mcp.json"), false)]);
    }

    #[test]
    fn agent_mcps_override_top_level_mcps() {
        let dir = create_temp_project();
        write_file(dir.path(), "BASE.md", "Base instructions");
        write_file(dir.path(), "base-mcp.json", "{}");
        write_file(dir.path(), "agent-mcp.json", "{}");

        let config = AetherSettings {
            prompts: vec![crate::PromptSource::file("BASE.md")],
            mcps: vec![McpSourceSpec::file("base-mcp.json")],
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                user_invocable: true,
                mcps: vec![McpSourceSpec::file("agent-mcp.json")],
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        let spec = catalog.resolve("planner").unwrap();

        assert_eq!(file_sources(&spec), vec![(dir.path().join("agent-mcp.json"), false)]);
    }

    #[test]
    fn missing_top_level_and_agent_prompts_still_errors() {
        let config = AetherSettings {
            agents: vec![AgentConfig {
                name: "planner".to_string(),
                description: "Planner agent".to_string(),
                model: "anthropic:claude-sonnet-4-5".to_string(),
                user_invocable: true,
                ..AgentConfig::default()
            }],
            ..AetherSettings::default()
        };

        let err = AgentCatalog::from_settings(Path::new("/tmp"), config).unwrap_err();

        assert!(matches!(err, SettingsError::NoPrompts { agent } if agent == "planner"));
    }

    #[test]
    fn resolve_missing_agent_returns_error() {
        let dir = create_temp_project();
        let catalog = create_test_catalog(dir.path().to_path_buf());
        let result = catalog.resolve("missing");
        assert!(matches!(result, Err(SettingsError::AgentNotFound { .. })));
    }

    #[test]
    fn resolve_preserves_agent_mcp() {
        let dir = create_temp_project();
        write_file(dir.path(), "agent-mcp.json", "{}");

        let mut planner = make_spec("planner", AgentSpecExposure::both());
        planner.mcp_config_sources = vec![McpConfigSource::direct(dir.path().join("agent-mcp.json"))];

        let catalog = AgentCatalog::new(dir.path().to_path_buf(), vec![planner], None);

        let spec = catalog.resolve("planner").unwrap();
        assert_eq!(file_sources(&spec), vec![(dir.path().join("agent-mcp.json"), false)]);
    }

    #[test]
    fn resolve_no_mcp_config_is_valid() {
        let dir = create_temp_project();
        let catalog =
            AgentCatalog::new(dir.path().to_path_buf(), vec![make_spec("planner", AgentSpecExposure::both())], None);

        let spec = catalog.resolve("planner").unwrap();
        assert!(spec.mcp_config_sources.is_empty());
    }
}
