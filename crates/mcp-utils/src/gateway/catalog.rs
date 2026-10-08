use super::{ToolExposure, ToolFilter, namespaced};
use crate::client::McpClient;
use rmcp::model::Tool;
use std::collections::BTreeMap;
use std::sync::Arc;
use utils::mcp_status::McpServerStatusEntry;

#[derive(Debug)]
pub struct McpCatalog {
    servers: Vec<Arc<CatalogEntry>>,
    settled: bool,
}

impl McpCatalog {
    pub fn tools(&self) -> Vec<Tool> {
        self.catalog_tools().filter(|tool| !tool.deferred).map(|tool| tool.tool.clone()).collect()
    }

    pub fn instructions(&self) -> BTreeMap<String, String> {
        self.servers
            .iter()
            .filter(|server| server.tools.iter().any(|tool| !tool.deferred))
            .filter_map(|server| Some((server.status.name.clone(), server.instructions.clone()?)))
            .collect()
    }

    pub fn has_deferred_tools(&self) -> bool {
        self.servers.iter().any(|server| server.has_deferred_tools())
    }

    pub fn statuses(&self) -> Vec<McpServerStatusEntry> {
        self.servers.iter().map(|server| server.status.clone()).collect()
    }

    pub(super) fn new(servers: Vec<Arc<CatalogEntry>>, settled: bool) -> Self {
        Self { servers, settled }
    }

    pub(super) fn is_settled(&self) -> bool {
        self.settled
    }

    pub(super) fn deferred_tools(&self) -> impl Iterator<Item = &Tool> {
        self.catalog_tools().filter(|tool| tool.deferred).map(|tool| &tool.tool)
    }

    pub(super) fn deferred_servers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.servers
            .iter()
            .filter(|server| server.has_deferred_tools())
            .map(|server| (server.status.name.as_str(), server.description.as_str()))
    }

    fn catalog_tools(&self) -> impl Iterator<Item = &CatalogTool> {
        self.servers.iter().flat_map(|server| &server.tools)
    }
}

#[derive(Debug)]
pub(super) struct CatalogEntry {
    pub(super) status: McpServerStatusEntry,
    description: String,
    instructions: Option<String>,
    tools: Vec<CatalogTool>,
}

impl CatalogEntry {
    pub(super) fn disconnected(status: McpServerStatusEntry) -> Self {
        Self { description: status.name.clone(), instructions: None, tools: Vec::new(), status }
    }

    pub(super) fn connected(
        status: McpServerStatusEntry,
        client: &McpClient,
        tools: &[Tool],
        exposure: &ToolExposure,
        filter: &ToolFilter,
    ) -> Self {
        let tools = tools
            .iter()
            .map(|tool| {
                let mut namespaced_tool = tool.clone();
                namespaced_tool.name = namespaced(&status.name, &tool.name).into();
                CatalogTool {
                    tool: namespaced_tool,
                    local_name: tool.name.to_string(),
                    deferred: exposure.defers(tool),
                }
            })
            .filter(|tool| filter.is_tool_allowed(&tool.tool))
            .collect();
        Self {
            description: client.description().unwrap_or_else(|| status.name.clone()),
            instructions: client.instructions(),
            tools,
            status,
        }
    }

    pub(super) fn tool(&self, name: &str) -> Option<&CatalogTool> {
        self.tools.iter().find(|tool| tool.tool.name == name)
    }

    fn has_deferred_tools(&self) -> bool {
        self.tools.iter().any(|tool| tool.deferred)
    }
}

#[derive(Debug)]
pub(super) struct CatalogTool {
    pub(super) local_name: String,
    pub(super) deferred: bool,
    tool: Tool,
}
