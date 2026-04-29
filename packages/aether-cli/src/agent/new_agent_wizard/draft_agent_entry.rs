use aether_project::{AetherSettings, AgentConfig, McpSourceSpec, PromptSource};
use std::{
    fs::{create_dir_all, read_to_string, write},
    path::{Path, PathBuf},
};

use super::new_agent_step::{NewAgentMode, PromptFile};
use crate::error::CliError;

pub struct DraftAgentEntry {
    pub entry: AgentConfig,
    pub system_md_content: String,
    pub system_md_edited: bool,
    pub selected_mcp_servers: Vec<String>,
    pub workspace_mcp_configs: Vec<String>,
}

impl DraftAgentEntry {
    pub fn slug(&self) -> String {
        self.entry.name.to_lowercase().replace(' ', "-")
    }

    pub fn generated_paths(&self, mode: &NewAgentMode) -> GeneratedPaths {
        let filename = format!("{}.md", self.slug().to_uppercase());
        match mode {
            NewAgentMode::ScaffoldProject => GeneratedPaths {
                system_md: PathBuf::from(format!(".aether/{filename}")),
                mcp_json: PathBuf::from(".aether/mcp.json"),
            },
            NewAgentMode::AddAgentToExistingProject => {
                let slug = self.slug();
                GeneratedPaths {
                    system_md: PathBuf::from(format!(".aether/agents/{slug}/{filename}")),
                    mcp_json: PathBuf::from(format!(".aether/agents/{slug}/mcp.json")),
                }
            }
        }
    }

    pub fn to_agent_config(&self, mode: &NewAgentMode, inherited_prompts: &[String]) -> AgentConfig {
        let paths = self.generated_paths(mode);

        let mut prompts = vec![PromptSource::file(paths.system_md.to_string_lossy())];
        match mode {
            NewAgentMode::ScaffoldProject => {
                prompts.extend(self.entry.prompts.iter().cloned());
            }
            NewAgentMode::AddAgentToExistingProject => {
                for prompt in &self.entry.prompts {
                    if let Some(path) = prompt.path()
                        && !inherited_prompts.iter().any(|d| d == path)
                    {
                        prompts.push(prompt.clone());
                    }
                }
            }
        }

        let mut mcp = if self.selected_mcp_servers.is_empty() {
            vec![]
        } else {
            vec![McpSourceSpec::file(paths.mcp_json.to_string_lossy())]
        };

        mcp.extend(self.workspace_mcp_configs.iter().map(McpSourceSpec::file));

        AgentConfig { prompts, mcps: mcp, ..self.entry.clone() }
    }

    pub fn to_settings(&self, mode: &NewAgentMode, existing: Option<&str>) -> AetherSettings {
        match mode {
            NewAgentMode::ScaffoldProject => {
                let entry = self.to_agent_config(mode, &[]);
                AetherSettings { agent: Some(entry.name.clone()), agents: vec![entry], ..AetherSettings::default() }
            }
            NewAgentMode::AddAgentToExistingProject => {
                let inherited = inherited_prompts_from_existing(existing);
                let entry = self.to_agent_config(mode, &inherited);

                let mut config: AetherSettings =
                    existing.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
                config.agents.push(entry);
                config
            }
        }
    }

    pub fn to_mcp_json(&self) -> String {
        use mcp_utils::client::config::{RawMcpConfig, RawMcpServerConfig};
        use std::collections::BTreeMap;

        let servers = self
            .selected_mcp_servers
            .iter()
            .map(|entry| {
                let name = entry.as_str();
                let args = match name {
                    "coding" => vec!["--rules-dir".into(), ".aether/skills".into()],
                    "skills" => {
                        vec!["--dir".into(), ".aether/skills".into(), "--notes-dir".into(), ".aether/notes".into()]
                    }
                    _ => vec![],
                };
                (name.to_string(), RawMcpServerConfig::InMemory { args, input: None })
            })
            .collect::<BTreeMap<_, _>>();

        let config = RawMcpConfig { servers };
        serde_json::to_string_pretty(&config).expect("mcp serialization cannot fail")
    }
}

pub struct GeneratedPaths {
    pub system_md: PathBuf,
    pub mcp_json: PathBuf,
}

fn inherited_prompts_from_existing(existing: Option<&str>) -> Vec<String> {
    existing
        .and_then(|s| serde_json::from_str::<AetherSettings>(s).ok())
        .map(|s| {
            let prompts = if s.prompts.is_empty() {
                s.agents.first().map(|agent| agent.prompts.clone()).unwrap_or_default()
            } else {
                s.prompts
            };

            prompts
                .iter()
                .filter_map(|p| p.path().map(str::to_string))
                .filter(|p| PromptFile::all().iter().any(|d| d.filename() == p))
                .collect()
        })
        .unwrap_or_default()
}

pub fn build_system_md(draft: &DraftAgentEntry) -> String {
    format!(
        "# {name}

{description}

## System Env

Working directory: !`pwd`\\
Platform: !`uname -s`\\
Today's date: !`date +%Y-%m-%d`\\
Git branch: !`git rev-parse --abbrev-ref HEAD`
",
        name = draft.entry.name,
        description = draft.entry.description,
    )
}

pub fn build_agents_md(draft: &DraftAgentEntry) -> String {
    format!("# {}\n\n{}\n\nYou are an expert coding assistant.\n", draft.entry.name, draft.entry.description)
}

pub fn scaffold(project_root: &Path, draft: &DraftAgentEntry) -> Result<(), CliError> {
    create_dir_all(project_root).map_err(CliError::IoError)?;

    let paths = draft.generated_paths(&NewAgentMode::ScaffoldProject);
    write_if_absent(&project_root.join(&paths.system_md), &draft.system_md_content)?;
    write_if_absent(&project_root.join(".aether/mcp.json"), &draft.to_mcp_json())?;
    if draft.entry.prompts.iter().any(|n| n.path() == Some(PromptFile::Agents.filename())) {
        write_if_absent(&project_root.join("AGENTS.md"), &build_agents_md(draft))?;
    }
    let config = draft.to_settings(&NewAgentMode::ScaffoldProject, None);
    let json = serde_json::to_string_pretty(&config).expect("settings serialization cannot fail");
    write_if_absent(&project_root.join(".aether/settings.json"), &json)?;

    Ok(())
}

pub fn add_agent(settings_path: &Path, draft: &DraftAgentEntry) -> Result<(), CliError> {
    let content = read_to_string(settings_path).map_err(CliError::IoError)?;
    let slug_dir = settings_path.parent().unwrap().join("agents").join(draft.slug());
    create_dir_all(&slug_dir).map_err(CliError::IoError)?;

    let filename = format!("{}.md", draft.slug().to_uppercase());
    write(slug_dir.join(filename), &draft.system_md_content).map_err(CliError::IoError)?;

    if !draft.selected_mcp_servers.is_empty() {
        write(slug_dir.join("mcp.json"), draft.to_mcp_json()).map_err(CliError::IoError)?;
    }

    let config = draft.to_settings(&NewAgentMode::AddAgentToExistingProject, Some(&content));
    let json = serde_json::to_string_pretty(&config).expect("settings serialization cannot fail");
    write(settings_path, json).map_err(CliError::IoError)?;

    Ok(())
}

fn write_if_absent(path: &Path, content: &str) -> Result<(), CliError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CliError::IoError)?;
    }
    std::fs::write(path, content).map_err(CliError::IoError)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ReasoningEffort;
    use mcp_utils::client::config::RawMcpConfig;

    fn has_prompt(agent: &AgentConfig, path: &str) -> bool {
        agent.prompts.iter().any(|prompt| prompt.path() == Some(path))
    }

    fn has_mcp(agent: &AgentConfig, path: &str) -> bool {
        agent.mcps.iter().any(|mcp| mcp.path() == Some(path))
    }

    fn default_draft() -> DraftAgentEntry {
        let mut draft = DraftAgentEntry {
            entry: AgentConfig {
                name: "Default".to_string(),
                description: "Default coding agent".to_string(),
                user_invocable: true,
                agent_invocable: true,
                model: "anthropic:claude-sonnet-4-5".to_string(),
                prompts: vec![PromptSource::file("AGENTS.md")],
                ..AgentConfig::default()
            },
            system_md_content: String::new(),
            system_md_edited: false,
            selected_mcp_servers: vec!["coding".into(), "skills".into(), "tasks".into()],
            workspace_mcp_configs: vec![],
        };
        draft.system_md_content = build_system_md(&draft);
        draft
    }

    fn researcher_draft() -> DraftAgentEntry {
        let mut draft = default_draft();
        draft.entry.name = "Researcher".to_string();
        draft.entry.description = "Research agent".to_string();
        draft.selected_mcp_servers = vec![];
        draft.workspace_mcp_configs = vec![];
        draft.system_md_content = build_system_md(&draft);
        draft
    }

    #[test]
    fn scaffold_writes_all_files() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        assert!(dir.path().join(".aether/settings.json").exists());
        assert!(dir.path().join(".aether/mcp.json").exists());
        assert!(dir.path().join(".aether/DEFAULT.md").exists());
        assert!(dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn scaffold_settings_json_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let config = aether_project::AetherSettings::load_default(dir.path()).unwrap();
        let catalog = aether_project::AgentCatalog::from_settings(dir.path(), config).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "Default");
    }

    #[test]
    fn scaffold_mcp_json_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let raw = RawMcpConfig::from_json_file(dir.path().join(".aether/mcp.json")).unwrap();
        assert_eq!(raw.servers.len(), 3);
        assert!(raw.servers.contains_key("coding"));
        assert!(raw.servers.contains_key("skills"));
        assert!(raw.servers.contains_key("tasks"));
    }

    #[test]
    fn scaffold_skips_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let agents_path = dir.path().join("AGENTS.md");
        std::fs::write(&agents_path, "My custom prompt").unwrap();

        scaffold(dir.path(), &default_draft()).unwrap();

        let content = std::fs::read_to_string(&agents_path).unwrap();
        assert_eq!(content, "My custom prompt");
    }

    #[test]
    fn scaffold_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep/nested/project");
        scaffold(&nested, &default_draft()).unwrap();

        assert!(nested.join(".aether/settings.json").exists());
        assert!(nested.join(".aether/mcp.json").exists());
        assert!(nested.join(".aether/DEFAULT.md").exists());
        assert!(nested.join("AGENTS.md").exists());
    }

    #[test]
    fn scaffold_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let draft = default_draft();
        scaffold(dir.path(), &draft).unwrap();
        scaffold(dir.path(), &draft).unwrap();
        assert!(dir.path().join(".aether/settings.json").exists());
    }

    #[test]
    fn generated_settings_reference_aether_paths() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();
        let agent = &config.agents[0];

        assert_eq!(config.agents.len(), 1);
        assert!(has_prompt(agent, ".aether/DEFAULT.md"));
        assert!(has_prompt(agent, "AGENTS.md"));
        assert!(has_mcp(agent, ".aether/mcp.json"));
    }

    #[test]
    fn scaffold_without_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.entry.prompts = vec![];
        scaffold(dir.path(), &draft).unwrap();

        assert!(!dir.path().join("AGENTS.md").exists());

        let content = std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();
        assert!(!has_prompt(&config.agents[0], "AGENTS.md"));
    }

    #[test]
    fn scaffold_includes_reasoning_effort() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.entry.reasoning_effort = Some(ReasoningEffort::High);
        scaffold(dir.path(), &draft).unwrap();

        let config = aether_project::AetherSettings::load_default(dir.path()).unwrap();
        let catalog = aether_project::AgentCatalog::from_settings(dir.path(), config).unwrap();
        assert_eq!(catalog.all()[0].reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn scaffold_omits_reasoning_effort_when_none() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        assert!(!content.contains("reasoningEffort"));
    }

    #[test]
    fn scaffold_custom_servers() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.selected_mcp_servers = vec!["coding".into(), "lsp".into()];
        scaffold(dir.path(), &draft).unwrap();

        let raw = RawMcpConfig::from_json_file(dir.path().join(".aether/mcp.json")).unwrap();
        assert_eq!(raw.servers.len(), 2);
        assert!(raw.servers.contains_key("coding"));
        assert!(raw.servers.contains_key("lsp"));
        assert!(!raw.servers.contains_key("tasks"));
    }

    #[test]
    fn scaffold_no_servers_no_mcp_json_ref() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.selected_mcp_servers = vec![];
        scaffold(dir.path(), &draft).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();
        assert!(config.agents[0].mcps.is_empty());
    }

    #[test]
    fn add_agent_appends_to_existing_settings() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        add_agent(&settings_path, &researcher_draft()).unwrap();

        let config = aether_project::AetherSettings::load_default(dir.path()).unwrap();
        let catalog = aether_project::AgentCatalog::from_settings(dir.path(), config).unwrap();
        assert_eq!(catalog.all().len(), 2);
        assert_eq!(catalog.all()[0].name, "Default");
        assert_eq!(catalog.all()[1].name, "Researcher");
    }

    #[test]
    fn add_agent_writes_per_agent_system_md() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        let mut new_draft = researcher_draft();
        new_draft.entry.prompts = vec![];
        let expected_per_agent = new_draft.system_md_content.clone();
        add_agent(&settings_path, &new_draft).unwrap();

        let agent_md = dir.path().join(".aether/agents/researcher/RESEARCHER.md");
        assert!(agent_md.exists());
        assert_eq!(std::fs::read_to_string(agent_md).unwrap(), expected_per_agent);
    }

    #[test]
    fn add_agent_writes_per_agent_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        let mut new_draft = researcher_draft();
        new_draft.entry.prompts = vec![];
        new_draft.selected_mcp_servers = vec!["coding".into(), "lsp".into()];
        add_agent(&settings_path, &new_draft).unwrap();

        let agent_mcp = dir.path().join(".aether/agents/researcher/mcp.json");
        assert!(agent_mcp.exists());

        let raw = RawMcpConfig::from_json_file(&agent_mcp).unwrap();
        assert_eq!(raw.servers.len(), 2);
        assert!(raw.servers.contains_key("coding"));
        assert!(raw.servers.contains_key("lsp"));
    }

    #[test]
    fn add_agent_config_references_local_assets() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        let mut new_draft = researcher_draft();
        new_draft.entry.user_invocable = false;
        new_draft.entry.prompts = vec![];
        new_draft.selected_mcp_servers = vec!["coding".into()];
        add_agent(&settings_path, &new_draft).unwrap();

        let content = std::fs::read_to_string(&settings_path).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();
        let researcher = &config.agents[1];

        assert_eq!(researcher.name, "Researcher");
        assert!(!researcher.user_invocable);
        assert!(researcher.agent_invocable);
        assert!(has_prompt(researcher, ".aether/agents/researcher/RESEARCHER.md"));
        assert!(has_mcp(researcher, ".aether/agents/researcher/mcp.json"));
    }

    #[test]
    fn generated_paths_scaffold() {
        let draft = default_draft();
        let paths = draft.generated_paths(&NewAgentMode::ScaffoldProject);
        assert_eq!(paths.system_md, PathBuf::from(".aether/DEFAULT.md"));
        assert_eq!(paths.mcp_json, PathBuf::from(".aether/mcp.json"));
    }

    #[test]
    fn generated_paths_add_agent() {
        let draft = default_draft();
        let paths = draft.generated_paths(&NewAgentMode::AddAgentToExistingProject);
        assert_eq!(paths.system_md, PathBuf::from(".aether/agents/default/DEFAULT.md"));
        assert_eq!(paths.mcp_json, PathBuf::from(".aether/agents/default/mcp.json"));
    }

    #[test]
    fn slug_from_name() {
        let mut draft = default_draft();
        draft.entry.name = "Codebase Explorer".to_string();
        assert_eq!(draft.slug(), "codebase-explorer");
    }

    #[test]
    fn build_system_md_uses_name_description_and_bash_block() {
        let mut draft = default_draft();
        draft.entry.name = "Researcher".to_string();
        draft.entry.description = "Digs through the codebase".to_string();
        let body = build_system_md(&draft);
        assert!(body.starts_with("# Researcher\n"));
        assert!(body.contains("Digs through the codebase"));
        assert!(body.contains("## System Env"));
        assert!(body.contains("Working directory: !`pwd`\\"));
        assert!(body.contains("Platform: !`uname -s`\\"));
        assert!(body.contains("Today's date: !`date +%Y-%m-%d`\\"));
        assert!(body.contains("Git branch: !`git rev-parse --abbrev-ref HEAD`"));
    }

    #[test]
    fn build_settings_scaffold_emits_all_selected_prompts() {
        let mut draft = default_draft();
        draft.entry.prompts = vec![PromptSource::file("AGENTS.md"), PromptSource::file("CLAUDE.md")];
        let config = draft.to_settings(&NewAgentMode::ScaffoldProject, None);
        let agent = &config.agents[0];

        assert!(has_prompt(agent, ".aether/DEFAULT.md"));
        assert!(has_prompt(agent, "AGENTS.md"));
        assert!(has_prompt(agent, "CLAUDE.md"));
    }

    #[test]
    fn build_settings_add_agent_skips_shared_prompts() {
        let existing = serde_json::to_string_pretty(&AetherSettings {
            agent: Some("Default".to_string()),
            prompts: vec![PromptSource::file("AGENTS.md")],
            agents: vec![AgentConfig {
                prompts: vec![],
                ..default_draft().to_agent_config(&NewAgentMode::ScaffoldProject, &[])
            }],
            ..AetherSettings::default()
        })
        .unwrap();

        let mut new_draft = researcher_draft();
        new_draft.entry.prompts = vec![PromptSource::file("AGENTS.md"), PromptSource::file("CLAUDE.md")];
        let config = new_draft.to_settings(&NewAgentMode::AddAgentToExistingProject, Some(&existing));

        let researcher = &config.agents[1];
        assert_eq!(researcher.name, "Researcher");
        assert!(!has_prompt(researcher, "AGENTS.md"));
        assert!(has_prompt(researcher, "CLAUDE.md"));
    }

    #[test]
    fn scaffold_writes_agents_md_when_selected() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.entry.prompts = vec![PromptSource::file("AGENTS.md")];
        scaffold(dir.path(), &draft).unwrap();
        assert!(dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn scaffold_includes_workspace_mcp_configs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mcp.json"), r#"{"servers":{}}"#).unwrap();

        let mut draft = default_draft();
        draft.workspace_mcp_configs = vec!["mcp.json".to_string()];
        scaffold(dir.path(), &draft).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();

        assert!(has_mcp(&config.agents[0], "mcp.json"));
    }

    #[test]
    fn add_agent_includes_workspace_mcp_configs() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        let mut new_draft = researcher_draft();
        new_draft.selected_mcp_servers = vec!["coding".into()];
        new_draft.workspace_mcp_configs = vec![".mcp.json".to_string()];
        add_agent(&settings_path, &new_draft).unwrap();

        let content = std::fs::read_to_string(&settings_path).unwrap();
        let config: AetherSettings = serde_json::from_str(&content).unwrap();
        let researcher = &config.agents[1];

        assert!(has_mcp(researcher, ".mcp.json"));
    }

    #[test]
    fn scaffold_never_writes_claude_or_gemini_md() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = default_draft();
        draft.entry.prompts =
            vec![PromptSource::file("AGENTS.md"), PromptSource::file("CLAUDE.md"), PromptSource::file("GEMINI.md")];
        scaffold(dir.path(), &draft).unwrap();

        assert!(dir.path().join("AGENTS.md").exists());
        assert!(!dir.path().join("CLAUDE.md").exists());
        assert!(!dir.path().join("GEMINI.md").exists());
    }
}
