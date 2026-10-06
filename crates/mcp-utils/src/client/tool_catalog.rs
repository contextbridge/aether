use super::{ToolExposure, naming::create_namespaced_tool_name, tool_filter::ToolFilter};
use rmcp::model::Tool;
use std::collections::BTreeMap;
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub const PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME: &str = "progressive-discovery";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDescription {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCatalog {
    servers: Vec<ServerCatalogEntry>,
    progressive_discovery_instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerCatalogEntry {
    name: String,
    description: String,
    instructions: Option<String>,
    status: McpServerStatus,
    auth_capability: McpServerAuthCapability,
    exposure: ToolExposure,
    tools: Vec<CatalogTool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogTools<'a> {
    pub model_visible: Vec<&'a CatalogTool>,
    pub deferred: Vec<&'a CatalogTool>,
}

/// An upstream tool as advertised by its server, renamed to `server__tool`.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogTool {
    server: String,
    local_name: String,
    tool: Tool,
    exposure: ToolExposureKind,
    allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposureKind {
    ModelVisible,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRoute {
    ModelVisible { namespaced_name: String },
    Deferred { server: String, tool: String },
}

impl ToolCatalog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn servers(&self) -> &[ServerCatalogEntry] {
        &self.servers
    }

    pub fn server(&self, name: &str) -> Option<&ServerCatalogEntry> {
        self.servers.iter().find(|server| server.name == name)
    }

    pub fn tool(&self, namespaced_name: &str) -> Option<&CatalogTool> {
        self.servers.iter().flat_map(|server| &server.tools).find(|tool| tool.namespaced_name() == namespaced_name)
    }

    pub fn tools(&self) -> CatalogTools<'_> {
        CatalogTools::from_tools(
            self.servers.iter().filter(|server| server.is_connected()).flat_map(|server| server.tools.iter()),
        )
    }

    pub fn tools_for(&self, server: &str) -> Option<CatalogTools<'_>> {
        self.server(server).map(|entry| {
            if entry.is_connected() { CatalogTools::from_tools(entry.tools.iter()) } else { CatalogTools::default() }
        })
    }

    pub fn discoverable_deferred_servers(&self) -> Vec<ServerDescription> {
        self.servers
            .iter()
            .filter(|server| {
                server.is_connected()
                    && server.tools.iter().any(|tool| tool.allowed && tool.exposure == ToolExposureKind::Deferred)
            })
            .map(|server| ServerDescription { name: server.name.clone(), description: server.description.clone() })
            .collect()
    }

    pub fn model_instructions(&self) -> BTreeMap<String, String> {
        let mut instructions = self
            .servers
            .iter()
            .filter(|server| server.is_connected())
            .filter(|server| {
                server.tools.iter().any(|tool| tool.allowed && tool.exposure == ToolExposureKind::ModelVisible)
            })
            .filter_map(|server| server.instructions.as_ref().map(|body| (server.name.clone(), body.clone())))
            .collect::<BTreeMap<_, _>>();
        if !self.discoverable_deferred_servers().is_empty()
            && let Some(body) = &self.progressive_discovery_instructions
        {
            instructions.insert(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string(), body.clone());
        }
        instructions
    }

    pub fn route_permitted(&self, route: &ToolRoute) -> bool {
        let (namespaced_name, exposure) = match route {
            ToolRoute::ModelVisible { namespaced_name } => (namespaced_name.clone(), ToolExposureKind::ModelVisible),
            ToolRoute::Deferred { server, tool } => {
                (create_namespaced_tool_name(server, tool), ToolExposureKind::Deferred)
            }
        };
        let Some(server) = self.servers.iter().find(|server| {
            server.is_connected() && server.tools.iter().any(|tool| tool.namespaced_name() == namespaced_name)
        }) else {
            return false;
        };
        let Some(tool) = server.tools.iter().find(|tool| tool.namespaced_name() == namespaced_name) else {
            return false;
        };
        tool.allowed && tool.exposure == exposure
    }

    pub fn server_statuses(&self) -> Vec<McpServerStatusEntry> {
        self.servers.iter().map(ServerCatalogEntry::status_entry).collect()
    }

    pub fn upsert_server(&mut self, entry: ServerCatalogEntry) {
        if let Some(existing) = self.servers.iter_mut().find(|server| server.name == entry.name) {
            *existing = entry;
        } else {
            self.servers.push(entry);
        }
    }

    pub fn remove_server(&mut self, name: &str) -> Option<ServerCatalogEntry> {
        self.servers.iter().position(|server| server.name == name).map(|index| self.servers.remove(index))
    }

    pub fn set_progressive_discovery_instructions(&mut self, instructions: Option<String>) {
        self.progressive_discovery_instructions = instructions;
    }
}

impl ServerCatalogEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        instructions: Option<String>,
        status: McpServerStatus,
        auth_capability: McpServerAuthCapability,
        exposure: ToolExposure,
        tools: &[Tool],
        filter: &ToolFilter,
    ) -> Self {
        let name = name.into();
        let tools = tools.iter().map(|tool| CatalogTool::new(&name, tool, &exposure, filter)).collect();
        Self { name, description: description.into(), instructions, status, auth_capability, exposure, tools }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }
    pub fn status(&self) -> &McpServerStatus {
        &self.status
    }
    pub fn auth_capability(&self) -> McpServerAuthCapability {
        self.auth_capability
    }
    pub fn exposure(&self) -> &ToolExposure {
        &self.exposure
    }
    pub fn tools(&self) -> &[CatalogTool] {
        &self.tools
    }
    pub fn status_entry(&self) -> McpServerStatusEntry {
        McpServerStatusEntry::new(&self.name, self.status.clone())
            .with_auth_capability(self.auth_capability)
            .with_deferred_tools(self.exposure.has_deferred_tools())
    }
    pub(crate) fn pending(name: impl Into<String>, exposure: ToolExposure) -> Self {
        let name = name.into();
        Self {
            description: name.clone(),
            name,
            instructions: None,
            status: McpServerStatus::Connecting,
            auth_capability: McpServerAuthCapability::Unavailable,
            exposure,
            tools: Vec::new(),
        }
    }
    pub(crate) fn with_status(&self, status: McpServerStatus, auth_capability: McpServerAuthCapability) -> Self {
        let mut next = self.clone();
        next.status = status;
        next.auth_capability = auth_capability;
        if !next.is_connected() {
            next.tools.clear();
            next.instructions = None;
        }
        next
    }
    fn is_connected(&self) -> bool {
        matches!(self.status, McpServerStatus::Connected { .. })
    }
}

impl<'a> CatalogTools<'a> {
    fn from_tools(tools: impl Iterator<Item = &'a CatalogTool>) -> Self {
        let mut partitioned = Self::default();
        for tool in tools.filter(|tool| tool.allowed) {
            match tool.exposure {
                ToolExposureKind::ModelVisible => partitioned.model_visible.push(tool),
                ToolExposureKind::Deferred => partitioned.deferred.push(tool),
            }
        }
        partitioned
    }
}

impl CatalogTool {
    pub fn namespaced_name(&self) -> &str {
        &self.tool.name
    }
    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn local_name(&self) -> &str {
        &self.local_name
    }
    /// The upstream tool definition, unchanged apart from its namespaced name.
    pub fn tool(&self) -> &Tool {
        &self.tool
    }
    pub fn exposure(&self) -> ToolExposureKind {
        self.exposure
    }
    pub fn allowed(&self) -> bool {
        self.allowed
    }
    pub fn route(&self) -> ToolRoute {
        match self.exposure {
            ToolExposureKind::ModelVisible => {
                ToolRoute::ModelVisible { namespaced_name: self.namespaced_name().to_string() }
            }
            ToolExposureKind::Deferred => {
                ToolRoute::Deferred { server: self.server.clone(), tool: self.local_name.clone() }
            }
        }
    }

    fn new(server: &str, tool: &Tool, exposure: &ToolExposure, filter: &ToolFilter) -> Self {
        let mut namespaced = tool.clone();
        namespaced.name = create_namespaced_tool_name(server, &tool.name).into();
        let exposure = if exposure.is_model_visible_tool(&tool.name) {
            ToolExposureKind::ModelVisible
        } else {
            ToolExposureKind::Deferred
        };
        Self {
            server: server.to_string(),
            local_name: tool.name.to_string(),
            allowed: filter.is_tool_allowed(&namespaced),
            tool: namespaced,
            exposure,
        }
    }
}
