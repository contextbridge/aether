use crate::llm::ToolDefinition;

use super::{
    ElicitationRequest, McpError, McpManager, McpServerConfig, ParseError, RawMcpConfig,
    ServerFactory, ServerInstructions, root_from_path,
    run_mcp_task::{McpCommand, run_mcp_task},
};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::{
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};

pub fn mcp() -> McpBuilder {
    McpBuilder::new()
}

/// Result of spawning MCP servers
pub struct McpSpawnResult {
    pub tool_definitions: Vec<ToolDefinition>,
    pub instructions: Vec<ServerInstructions>,
    pub command_tx: Sender<McpCommand>,
    pub handle: JoinHandle<()>,
}

pub struct McpBuilder {
    mcp_configs: Vec<McpServerConfig>,
    factories: HashMap<String, ServerFactory>,
    mcp_channel_capacity: usize,
    roots: Vec<PathBuf>,
}

impl Default for McpBuilder {
    fn default() -> Self {
        Self {
            mcp_configs: Vec::new(),
            factories: HashMap::new(),
            mcp_channel_capacity: 1000,
            roots: Vec::new(),
        }
    }
}

impl McpBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_servers(mut self, configs: Vec<McpServerConfig>) -> Self {
        self.mcp_configs.extend(configs);
        self
    }

    pub fn register_in_memory_server(
        mut self,
        name: impl Into<String>,
        factory: ServerFactory,
    ) -> Self {
        self.factories.insert(name.into(), factory);
        self
    }

    /// Set workspace roots that will be advertised to MCP servers.
    ///
    /// These roots are passed to the MCP client via the roots protocol,
    /// allowing servers to dynamically receive workspace information without
    /// relying solely on CLI arguments.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = roots;
        self
    }

    pub async fn from_json_file(mut self, path: &str) -> Result<Self, ParseError> {
        let raw_config = RawMcpConfig::from_json_file(path)?;
        let mcp_configs = raw_config.into_configs(&self.factories).await?;
        self.mcp_configs.extend(mcp_configs);
        Ok(self)
    }

    pub async fn spawn(self) -> Result<McpSpawnResult, McpError> {
        let (mcp_command_tx, mcp_command_rx) =
            mpsc::channel::<McpCommand>(self.mcp_channel_capacity);
        let (elicitation_tx, _elicitation_rx) =
            mpsc::channel::<ElicitationRequest>(self.mcp_channel_capacity);

        let mut mcp_manager = McpManager::new(elicitation_tx);
        mcp_manager.add_mcps(self.mcp_configs).await?;

        // Set workspace roots if provided
        if !self.roots.is_empty() {
            let roots = self
                .roots
                .into_iter()
                .map(|path| root_from_path(path, None))
                .collect();
            mcp_manager.set_roots(roots).await?;
        }

        let tool_definitions = mcp_manager.tool_definitions();
        let instructions = mcp_manager.server_instructions();
        let mcp_handle = tokio::spawn(run_mcp_task(mcp_manager, mcp_command_rx));

        Ok(McpSpawnResult {
            tool_definitions,
            instructions,
            command_tx: mcp_command_tx,
            handle: mcp_handle,
        })
    }
}
