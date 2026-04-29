use aether_project::{AetherConfig, AgentConfig};
use crossterm::style::Stylize;
use std::fs;
use std::path::Path;

use crate::agent::RemoveArgs;
use crate::error::CliError;

pub fn run_remove(args: RemoveArgs) -> Result<(), CliError> {
    let project_root = args.path.canonicalize().unwrap_or(args.path);
    let settings_path = project_root.join(".aether/settings.json");

    let content = fs::read_to_string(&settings_path).map_err(CliError::IoError)?;
    let mut config: AetherConfig =
        serde_json::from_str(&content).map_err(|e| CliError::AgentError(format!("Failed to parse settings: {e}")))?;

    let index = config
        .agents
        .iter()
        .position(|a| a.name == args.name)
        .ok_or_else(|| CliError::AgentError(format!("Agent '{}' not found", args.name)))?;

    let entry = config.agents.remove(index);
    let slug = entry.name.to_lowercase().replace(' ', "-");

    cleanup_agent_files(&project_root, &slug, &entry);

    let json = serde_json::to_string_pretty(&config).expect("settings serialization cannot fail");
    fs::write(&settings_path, json).map_err(CliError::IoError)?;

    println!("{} Removed agent '{}'", "✓".green().bold(), entry.name);
    Ok(())
}

fn cleanup_agent_files(project_root: &Path, slug: &str, entry: &AgentConfig) {
    let per_agent_dir = project_root.join(".aether/agents").join(slug);
    if per_agent_dir.is_dir() {
        let _ = fs::remove_dir_all(&per_agent_dir);
    }

    for prompt in &entry.prompts {
        let Some(prompt_path) = prompt.path() else { continue };
        let path = project_root.join(prompt_path);
        if path.starts_with(project_root.join(".aether")) {
            let _ = fs::remove_file(&path);
        }
    }

    for mcp in &entry.mcps {
        let Some(mcp_path) = mcp.path() else { continue };
        let path = project_root.join(mcp_path);
        if path.starts_with(project_root.join(".aether")) {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::new_agent_wizard::{DraftAgentEntry, add_agent, build_system_md, scaffold};

    #[test]
    fn remove_only_agent() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let args = super::super::RemoveArgs { name: "Default".to_string(), path: dir.path().to_path_buf() };
        run_remove(args).unwrap();

        let content = fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap();
        let config: AetherConfig = serde_json::from_str(&content).unwrap();
        assert!(config.agents.is_empty());

        assert!(!dir.path().join(".aether/DEFAULT.md").exists());
    }

    #[test]
    fn remove_second_agent_keeps_first() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let settings_path = dir.path().join(".aether/settings.json");
        add_agent(&settings_path, &researcher_draft()).unwrap();

        let args = super::super::RemoveArgs { name: "Researcher".to_string(), path: dir.path().to_path_buf() };
        run_remove(args).unwrap();

        let content = fs::read_to_string(&settings_path).unwrap();
        let config: AetherConfig = serde_json::from_str(&content).unwrap();
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.agents[0].name, "Default");

        assert!(!dir.path().join(".aether/agents/researcher").exists());
        assert!(dir.path().join(".aether/DEFAULT.md").exists());
    }

    #[test]
    fn remove_nonexistent_agent_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &default_draft()).unwrap();

        let args = super::super::RemoveArgs { name: "Ghost".to_string(), path: dir.path().to_path_buf() };
        let result = run_remove(args);
        assert!(result.is_err());
    }

    #[test]
    fn remove_no_settings_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let args = super::super::RemoveArgs { name: "Default".to_string(), path: dir.path().to_path_buf() };
        let result = run_remove(args);
        assert!(result.is_err());
    }

    fn default_draft() -> DraftAgentEntry {
        let mut draft = DraftAgentEntry {
            entry: AgentConfig {
                name: "Default".to_string(),
                description: "Default coding agent".to_string(),
                user_invocable: true,
                agent_invocable: true,
                model: "anthropic:claude-sonnet-4-5".to_string(),
                prompts: vec![aether_project::PromptSource::file("AGENTS.md")],
                ..AgentConfig::default()
            },
            system_md_content: String::new(),
            system_md_edited: false,
            selected_mcp_servers: vec!["coding".into()],
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
}
