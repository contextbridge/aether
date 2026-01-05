use aether::agent::{AgentHandle, AgentMessage, Prompt, UserMessage, agent};
use aether::llm::provider::StreamingModelProvider;
use aether::mcp::McpSpawnResult;
use aether::mcp::config::{RawMcpConfig, RawMcpServerConfig};
use aether::mcp::mcp;
use aether::mcp::run_mcp_task::McpCommand;
use agent_client_protocol as acp;
use futures::FutureExt;
use mcp_lexicon::{CodingMcp, CodingMcpArgs, LspCodingTools, PluginsMcp, ServiceExt, TasksMcp};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::acp_actor::AcpActorHandle;
use crate::acp_coding_tools::AcpCodingTools;
use crate::mappers::map_mcp_prompt_to_available_command;

/// Represents an active Aether agent session
pub struct Session {
    pub id: String,
    pub agent_tx: mpsc::Sender<UserMessage>,
    pub agent_rx: mpsc::Receiver<AgentMessage>,
    pub _agent_handle: AgentHandle,
    pub _mcp_handle: JoinHandle<()>,
    pub cancel_flag: Arc<AtomicBool>,
    pub mcp_tx: mpsc::Sender<McpCommand>,
}

impl Session {
    /// Creates a new session with the given LLM provider and configuration
    pub async fn new<T: StreamingModelProvider + 'static>(
        id: String,
        llm: T,
        system_prompt: Option<String>,
        mcp_config_path: std::path::PathBuf,
        actor_handle: AcpActorHandle,
        acp_session_id: acp::SessionId,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        debug!("Creating new session: {}", id);
        debug!("Loading MCP configuration from: {:?}", mcp_config_path);

        let project_path = parse_coding_root_dir(&mcp_config_path)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        debug!("Using project root for LSP: {:?}", project_path);

        let config_str = mcp_config_path.to_str().ok_or("Invalid MCP config path")?;
        let tasks_project_path = project_path.clone();
        let roots_path = project_path.clone();

        debug!("Creating ACP-enabled CodingMcp and PluginsMcp");
        let McpSpawnResult {
            tool_definitions,
            instructions,
            command_tx: mcp_tx,
            handle: mcp_handle,
        } = mcp()
            .register_in_memory_server(
                "coding",
                Box::new(move |_args| {
                    let actor_handle = actor_handle.clone();
                    let acp_session_id = acp_session_id.clone();
                    let project_path = project_path.clone();
                    async move {
                        let acp_tools =
                            AcpCodingTools::new(actor_handle.clone(), acp_session_id.clone());
                        let lsp_tools = LspCodingTools::new(acp_tools, project_path.clone());
                        debug!("LspCodingTools created with lazy LSP spawning");
                        CodingMcp::with_tools(lsp_tools)
                            .with_root_dir(project_path)
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .register_in_memory_server(
                "plugins",
                Box::new(|args| {
                    async move {
                        PluginsMcp::from_args(args)
                            .expect("Failed to parse PluginsMcp args")
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .register_in_memory_server(
                "tasks",
                Box::new(move |args| {
                    let project_path = tasks_project_path.clone();
                    async move {
                        TasksMcp::from_args(args)
                            .unwrap_or_else(|e| {
                                tracing::warn!(
                                    "Failed to parse TasksMcp args: {e}, using defaults"
                                );
                                TasksMcp::new(project_path)
                            })
                            .into_dyn()
                    }
                    .boxed()
                }),
            )
            .with_roots(vec![roots_path])
            .from_json_file(config_str)
            .await?
            .spawn()
            .await?;

        // Build system prompt from AGENTS.md and optional custom prompt
        let mut prompts = vec![Prompt::agents_md()];
        if let Some(ref custom_prompt) = system_prompt {
            prompts.push(Prompt::text(custom_prompt));
        }

        let system_prompt_text = Prompt::build_all(&prompts)
            .map_err(|e| format!("Failed to build system prompt: {e}"))?;

        let builder = agent(llm)
            .system(&system_prompt_text)
            .prompt(Prompt::mcp_instructions(instructions))
            .tools(mcp_tx.clone(), tool_definitions);

        let (agent_tx, agent_rx, agent_handle) = builder.spawn().await?;

        debug!("Session {} created successfully", id);

        Ok(Self {
            id,
            agent_tx,
            agent_rx,
            _agent_handle: agent_handle,
            _mcp_handle: mcp_handle,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            mcp_tx,
        })
    }

    /// Sends a text prompt to the agent
    pub async fn send_prompt(&self, text: String) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Sending prompt to session {}", self.id);
        self.agent_tx
            .send(UserMessage::text(&text))
            .await
            .map_err(|e| format!("Failed to send prompt: {e}"))?;
        Ok(())
    }

    /// Cancels any ongoing prompt processing
    pub async fn cancel(&self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Cancelling session {}", self.id);
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.agent_tx
            .send(UserMessage::Cancel)
            .await
            .map_err(|e| format!("Failed to send cancel: {e}"))?;
        Ok(())
    }

    /// Receives the next agent message
    pub async fn recv(&mut self) -> Option<AgentMessage> {
        self.agent_rx.recv().await
    }

    /// Lists available slash commands by querying MCP prompts
    pub async fn list_available_commands(
        &self,
    ) -> Result<Vec<acp::AvailableCommand>, Box<dyn std::error::Error>> {
        let (tx, rx) = oneshot::channel();

        self.mcp_tx
            .send(McpCommand::ListPrompts { tx })
            .await
            .map_err(|e| format!("Failed to send ListPrompts command: {e}"))?;

        let prompts = rx
            .await
            .map_err(|e| format!("Failed to receive prompts: {e}"))??;

        let commands = prompts
            .iter()
            .map(map_mcp_prompt_to_available_command)
            .collect();

        Ok(commands)
    }

    /// Expands a slash command by fetching the corresponding MCP prompt
    pub async fn expand_slash_command(
        &self,
        command_name: &str,
        args_text: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let arguments = parse_slash_command_arguments(args_text);

        // Try to find the prompt by querying all available prompts
        // We need to map the command name back to its namespaced form
        let (tx_list, rx_list) = oneshot::channel();
        self.mcp_tx
            .send(McpCommand::ListPrompts { tx: tx_list })
            .await
            .map_err(|e| format!("Failed to send ListPrompts command: {e}"))?;

        let prompts = rx_list
            .await
            .map_err(|e| format!("Failed to receive prompts: {e}"))??;

        // Find the prompt that matches the command name
        let matching_prompt = prompts
            .iter()
            .find(|p| {
                // Extract the base name from the namespaced prompt name
                p.name.split("__").last().unwrap_or("") == command_name
            })
            .ok_or_else(|| format!("Slash command '{command_name}' not found"))?;

        let namespaced_name = matching_prompt.name.to_string();

        // Get the prompt content
        let (tx_get, rx_get) = oneshot::channel();
        self.mcp_tx
            .send(McpCommand::GetPrompt {
                name: namespaced_name.clone(),
                arguments,
                tx: tx_get,
            })
            .await
            .map_err(|e| format!("Failed to send GetPrompt command: {e}"))?;

        let prompt_result = rx_get
            .await
            .map_err(|e| format!("Failed to receive prompt: {e}"))??;

        // Extract text content from the first message
        if let Some(message) = prompt_result.messages.first() {
            match &message.content {
                rmcp::model::PromptMessageContent::Text { text } => Ok(text.to_string()),
                _ => Err("Prompt message does not contain text content".into()),
            }
        } else {
            Err("Prompt result contains no messages".into())
        }
    }
}

/// Parse slash command arguments into a map with both positional and special variables.
///
/// Creates an argument map with:
/// - "ARGUMENTS": The full argument string
/// - "1", "2", "3", etc.: Individual positional arguments (1-based)
fn parse_slash_command_arguments(
    args_text: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    if args_text.is_empty() {
        None
    } else {
        let mut arg_map = serde_json::Map::new();

        // Add special ARGUMENTS variable with all args as a single string
        arg_map.insert(
            "ARGUMENTS".to_string(),
            serde_json::Value::String(args_text.to_string()),
        );

        // Add positional parameters (1-based)
        for (i, arg) in args_text.split_whitespace().enumerate() {
            arg_map.insert(
                (i + 1).to_string(),
                serde_json::Value::String(arg.to_string()),
            );
        }

        Some(arg_map)
    }
}

/// Parse the root directory from the mcp.json coding server args.
///
/// Looks for the "coding" server entry and parses its args for `--root-dir`.
/// Relative paths (like ".") are resolved against the mcp.json's directory.
/// Returns None if the config doesn't exist, doesn't have a coding server,
/// or the coding server doesn't specify a root-dir.
fn parse_coding_root_dir(mcp_config_path: &std::path::Path) -> Option<PathBuf> {
    let raw_config = RawMcpConfig::from_json_file(mcp_config_path).ok()?;
    let coding_config = raw_config.servers.get("coding")?;

    if let RawMcpServerConfig::InMemory { args } = coding_config {
        let parsed_args = CodingMcpArgs::from_args(args.clone()).ok()?;
        let root_dir = parsed_args.root_dir?;

        // Resolve relative paths against the mcp.json's directory
        if root_dir.is_relative() {
            let config_dir = mcp_config_path.parent()?;
            Some(config_dir.join(&root_dir).canonicalize().ok()?)
        } else {
            Some(root_dir)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{File, create_dir_all, remove_dir, remove_file};

    use serde_json::Map;
    use serde_json::Value;

    use super::*;

    #[test]
    fn test_argument_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let args_text = "do a thing that has spaces";
        let arg_map = parse_slash_command_arguments(args_text).ok_or("Expected Some, got None")?;
        let expected = Map::from_iter([
            (
                "ARGUMENTS".to_string(),
                Value::String("do a thing that has spaces".to_string()),
            ),
            ("1".to_string(), Value::String("do".to_string())),
            ("2".to_string(), Value::String("a".to_string())),
            ("3".to_string(), Value::String("thing".to_string())),
            ("4".to_string(), Value::String("that".to_string())),
            ("5".to_string(), Value::String("has".to_string())),
            ("6".to_string(), Value::String("spaces".to_string())),
        ]);

        assert_eq!(arg_map, expected);

        Ok(())
    }

    #[test]
    fn test_parse_coding_root_dir_resolves_relative_path() {
        let (temp_dir, mcp_json_path) = create_temp_mcp_json("relative_path", ".");
        let expected_path = temp_dir.canonicalize().unwrap();
        let resolved = parse_coding_root_dir(&mcp_json_path).expect("Should resolve relative path");

        assert_eq!(resolved, expected_path);
        remove_file(&mcp_json_path).ok();
        remove_dir(&temp_dir).ok();
    }

    #[test]
    fn test_parse_coding_root_dir_preserves_absolute_path() {
        let absolute_path = std::env::temp_dir().canonicalize().unwrap();
        let (temp_dir, mcp_json_path) =
            create_temp_mcp_json("absolute_path", &absolute_path.display().to_string());
        let resolved = parse_coding_root_dir(&mcp_json_path).expect("Should return absolute path");

        assert_eq!(resolved, absolute_path);
        remove_file(&mcp_json_path).ok();
        remove_dir(&temp_dir).ok();
    }

    fn create_temp_mcp_json(name: &str, root_dir: &str) -> (PathBuf, PathBuf) {
        use std::io::Write;

        let temp_dir = std::env::temp_dir().join(format!("aether_test_{name}"));
        create_dir_all(&temp_dir).unwrap();

        let mcp_json_path = temp_dir.join("mcp.json");
        let mut file = File::create(&mcp_json_path).unwrap();
        write!(
            file,
            r#"{{
                "servers": {{
                    "coding": {{
                        "type": "in-memory",
                        "args": ["--root-dir", "{root_dir}"]
                    }}
                }}
            }}"#
        )
        .unwrap();

        (temp_dir, mcp_json_path)
    }
}
