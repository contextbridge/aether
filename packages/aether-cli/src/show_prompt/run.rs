use std::collections::BTreeMap;

use super::PromptArgs;
use crate::error::CliError;
use crate::resolve::resolve_agent_spec;
use crate::runtime::RuntimeBuilder;
use aether_core::core::Prompt;
use aether_project::{AetherSettings, AgentCatalog};
use llm::ToolDefinition;
use serde_json::Value;

pub async fn run_prompt(args: PromptArgs) -> Result<(), CliError> {
    let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
    let config = if let Some(source) = args.settings_source.source() {
        AetherSettings::load(&cwd, [source])
    } else {
        AetherSettings::load_default(&cwd)
    }
    .map_err(|e| CliError::AgentError(e.to_string()))?;
    let catalog = if config.agents.is_empty() {
        AgentCatalog::empty(cwd.clone())
    } else {
        AgentCatalog::from_settings(&cwd, config).map_err(|e| CliError::AgentError(e.to_string()))?
    };
    let spec = resolve_agent_spec(&catalog, args.agent.as_deref())?;

    let info = RuntimeBuilder::from_spec(cwd.clone(), spec)
        .mcp_sources(args.mcp_config.sources(&cwd))
        .build_prompt_info()
        .await?;

    let system_prompt = build_prompt(&info.spec.prompts, args.system_prompt.as_deref()).await?;
    let tools_output = build_tools(&info.tool_definitions);

    println!("{system_prompt}");

    if !tools_output.is_empty() {
        println!();
        println!("--- Tools ({} tools) ---", info.tool_definitions.len());
        println!();
        println!("{tools_output}");
    }

    println!();
    println!("{}", format_stats(system_prompt.len(), tools_output.len(), info.tool_definitions.len()));

    Ok(())
}

pub async fn build_prompt(prompts: &[Prompt], custom: Option<&str>) -> Result<String, CliError> {
    let mut prompts = prompts.to_vec();
    if let Some(custom) = custom {
        prompts.push(Prompt::text(custom));
    }
    Prompt::build_all(&prompts).await.map_err(|e| CliError::AgentError(e.to_string()))
}

pub fn build_tools(tools: &[ToolDefinition]) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut grouped: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for tool in tools {
        let server = tool.server.as_deref().unwrap_or("(built-in)");
        let input_schema = serde_json::from_str::<Value>(&tool.parameters).unwrap_or(Value::Null);
        let entry = serde_json::json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": input_schema,
        });
        grouped.entry(server).or_default().push(entry);
    }

    let mut sections = Vec::new();
    for (server, entries) in &grouped {
        let json = serde_json::to_string_pretty(entries).unwrap_or_default();
        sections.push(format!("Server: {server}\n{json}"));
    }

    sections.join("\n\n")
}

pub fn format_stats(prompt_chars: usize, tool_schema_chars: usize, tool_count: usize) -> String {
    let est_tokens = (prompt_chars + tool_schema_chars) / 4;
    format!(
        "---\n\
         Prompt chars:     {prompt_chars:>8}\n\
         Tool schema chars:{tool_schema_chars:>8}\n\
         Est. tokens:     ~{est_tokens:>8}\n\
         MCP tools:        {tool_count:>8}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, desc: &str, params: &str, server: Option<&str>) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: desc.to_string(),
            parameters: params.to_string(),
            server: server.map(String::from),
        }
    }

    #[test]
    fn format_stats_computes_token_estimate() {
        let output = format_stats(12000, 8500, 14);
        assert_eq!(
            output,
            "---\n\
             Prompt chars:        12000\n\
             Tool schema chars:    8500\n\
             Est. tokens:     ~    5125\n\
             MCP tools:              14"
        );
    }

    #[test]
    fn format_stats_handles_zero() {
        let output = format_stats(0, 0, 0);
        assert_eq!(
            output,
            "---\n\
             Prompt chars:            0\n\
             Tool schema chars:       0\n\
             Est. tokens:     ~       0\n\
             MCP tools:               0"
        );
    }

    #[test]
    fn format_stats_handles_small_values() {
        let output = format_stats(3, 0, 1);
        assert_eq!(
            output,
            "---\n\
             Prompt chars:            3\n\
             Tool schema chars:       0\n\
             Est. tokens:     ~       0\n\
             MCP tools:               1"
        );
    }

    #[test]
    fn build_tools_groups_by_server() {
        let tools = vec![
            tool("fs_read", "Read a file", r#"{"type":"object"}"#, Some("filesystem")),
            tool("git_log", "Show log", r#"{"type":"object"}"#, Some("git")),
            tool("fs_write", "Write a file", r#"{"type":"object"}"#, Some("filesystem")),
        ];
        let output = build_tools(&tools);
        // BTreeMap sorts: filesystem < git
        let fs_pos = output.find("Server: filesystem").unwrap();
        let git_pos = output.find("Server: git").unwrap();
        assert!(fs_pos < git_pos);
        // filesystem group has both tools
        assert!(output.contains("fs_read"));
        assert!(output.contains("fs_write"));
    }

    #[test]
    fn build_tools_handles_no_server() {
        let tools = vec![tool("builtin_tool", "A built-in", r#"{"type":"object"}"#, None)];
        let output = build_tools(&tools);
        assert!(output.contains("Server: (built-in)"));
        assert!(output.contains("builtin_tool"));
    }

    #[test]
    fn build_tools_produces_api_format() {
        let tools = vec![tool("my_tool", "Does stuff", r#"{"type":"object","properties":{}}"#, Some("test"))];
        let output = build_tools(&tools);
        // Strip "Server: test\n" prefix to get the JSON
        let json_start = output.find('[').unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&output[json_start..]).unwrap();
        assert_eq!(parsed.len(), 1);
        let entry = &parsed[0];
        assert_eq!(entry["name"], "my_tool");
        assert_eq!(entry["description"], "Does stuff");
        assert!(entry["input_schema"].is_object());
    }

    #[test]
    fn build_tools_empty() {
        assert_eq!(build_tools(&[]), "");
    }

    #[test]
    fn build_tools_malformed_params() {
        let tools = vec![tool("bad_tool", "Broken params", "not valid json", Some("srv"))];
        let output = build_tools(&tools);
        assert!(output.contains("bad_tool"));
        assert!(output.contains("null"));
    }
}
