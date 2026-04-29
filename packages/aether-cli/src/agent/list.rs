use aether_project::{AetherSettings, AgentConfig};
use crossterm::style::Stylize;
use std::fs;

use crate::agent::ListArgs;
use crate::error::CliError;

pub fn run_list(args: ListArgs) -> Result<(), CliError> {
    let project_root = args.path.canonicalize().unwrap_or(args.path);
    let settings_path = project_root.join(".aether/settings.json");

    let content = match fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("No agents found. Run `aether agent new` to create one.");
            return Ok(());
        }
        Err(e) => return Err(CliError::IoError(e)),
    };

    let config: AetherSettings =
        serde_json::from_str(&content).map_err(|e| CliError::AgentError(format!("Failed to parse settings: {e}")))?;

    if config.agents.is_empty() {
        println!("No agents found. Run `aether agent new` to create one.");
        return Ok(());
    }

    let mut sorted: Vec<&AgentConfig> = config.agents.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    for (i, agent) in sorted.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_agent(agent);
    }

    Ok(())
}

fn print_agent(agent: &AgentConfig) {
    println!("{}", agent.name.as_str().bold().cyan());
    println!("  {}       {}", "model:".dim(), agent.model);

    let reasoning = agent.reasoning_effort.as_ref().map_or("none".to_string(), std::string::ToString::to_string);
    println!("  {}   {reasoning}", "reasoning:".dim());

    println!("  {} {}", "description:".dim(), agent.description);

    let mut surfaces = Vec::new();
    if agent.user_invocable {
        surfaces.push("user");
    }
    if agent.agent_invocable {
        surfaces.push("agent");
    }
    println!("  {}   {}", "invocable:".dim(), surfaces.join(", "));

    if !agent.prompts.is_empty() {
        println!(
            "  {}     {}",
            "prompts:".dim(),
            agent.prompts.iter().filter_map(aether_project::PromptSource::path).collect::<Vec<_>>().join(", ")
        );
    }

    if !agent.mcps.is_empty() {
        println!(
            "  {} {}",
            "mcp servers:".dim(),
            agent.mcps.iter().filter_map(aether_project::McpSourceSpec::path).collect::<Vec<_>>().join(", ")
        );
    }

    if !agent.tools.allow.is_empty() {
        println!("  {} {}", "tools allow:".dim(), agent.tools.allow.join(", "));
    }
    if !agent.tools.deny.is_empty() {
        println!("  {}  {}", "tools deny:".dim(), agent.tools.deny.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::new_agent_wizard::{DraftAgentEntry, build_system_md, scaffold};

    #[test]
    fn list_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let args = super::super::ListArgs { path: dir.path().to_path_buf() };
        run_list(args).unwrap();
    }

    #[test]
    fn list_project_with_agents() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &test_draft()).unwrap();
        let args = super::super::ListArgs { path: dir.path().to_path_buf() };
        run_list(args).unwrap();
    }

    fn test_draft() -> DraftAgentEntry {
        let mut draft = DraftAgentEntry {
            entry: AgentConfig {
                name: "Coder".to_string(),
                description: "A coding agent".to_string(),
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
}
