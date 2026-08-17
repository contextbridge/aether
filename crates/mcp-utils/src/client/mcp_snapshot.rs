use super::{
    McpClient, McpError, ToolCatalog, ToolRoute,
    naming::{create_namespaced_tool_name, split_on_server_name},
};
use llm::ToolDefinition;
use rmcp::{RoleClient, model::CallToolRequestParams, service::RunningService};
use serde_json::{Map, Value};
use std::{collections::HashMap, fmt, sync::Arc};

#[derive(Clone, Default)]
pub struct McpSnapshot {
    catalog: Arc<ToolCatalog>,
    clients: Arc<HashMap<String, Arc<RunningService<RoleClient, McpClient>>>>,
}

impl fmt::Debug for McpSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpSnapshot")
            .field("catalog", &self.catalog)
            .field("connected_servers", &self.clients.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl McpSnapshot {
    pub fn new(
        catalog: Arc<ToolCatalog>,
        clients: Arc<HashMap<String, Arc<RunningService<RoleClient, McpClient>>>>,
    ) -> Self {
        Self { catalog, clients }
    }

    pub fn catalog(&self) -> &Arc<ToolCatalog> {
        &self.catalog
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions =
            self.catalog.tools().model_visible.into_iter().map(|tool| tool.definition().clone()).collect::<Vec<_>>();
        if !self.catalog.discoverable_deferred_servers().is_empty() {
            definitions.insert(0, super::tool_proxy::call_tool_definition());
        }
        definitions
    }

    pub fn model_instructions(&self) -> std::collections::BTreeMap<String, String> {
        self.catalog.model_instructions()
    }

    pub fn server_statuses(&self) -> Vec<crate::status::McpServerStatusEntry> {
        self.catalog.server_statuses()
    }

    pub fn resolve(
        &self,
        route: ToolRoute,
        arguments: Map<String, Value>,
    ) -> super::Result<(Arc<RunningService<RoleClient, McpClient>>, CallToolRequestParams)> {
        if let ToolRoute::ModelVisible { namespaced_name } = &route {
            split_on_server_name(namespaced_name)
                .ok_or_else(|| McpError::InvalidToolNameFormat(namespaced_name.clone()))?;
        }
        if !self.catalog.route_permitted(&route) {
            let (tool_name, namespaced_name) = match &route {
                ToolRoute::ModelVisible { namespaced_name } => (namespaced_name.clone(), namespaced_name.clone()),
                ToolRoute::Deferred { server, tool } => (tool.clone(), create_namespaced_tool_name(server, tool)),
            };
            if matches!(route, ToolRoute::Deferred { .. })
                && self.catalog.route_permitted(&ToolRoute::ModelVisible { namespaced_name: namespaced_name.clone() })
            {
                return Err(McpError::DirectToolRequiresDirectRoute { tool_name, direct_name: namespaced_name });
            }
            return Err(McpError::ToolNotFound(namespaced_name));
        }
        let (server, tool) = match route {
            ToolRoute::ModelVisible { namespaced_name } => {
                let (server, tool) =
                    split_on_server_name(&namespaced_name).expect("model-visible route was validated above");
                (server.to_string(), tool.to_string())
            }
            ToolRoute::Deferred { server, tool } => (server, tool),
        };
        let client = self.clients.get(&server).cloned().ok_or_else(|| McpError::ServerNotFound(server.clone()))?;
        Ok((client, CallToolRequestParams::new(tool).with_arguments(arguments)))
    }

    pub fn clients_with_prompts(&self) -> Vec<(String, Arc<RunningService<RoleClient, McpClient>>)> {
        self.clients
            .iter()
            .filter(|(_, client)| client.peer_info().is_some_and(|info| info.capabilities.prompts.is_some()))
            .map(|(name, client)| (name.clone(), Arc::clone(client)))
            .collect()
    }

    pub fn client_for_prompt(
        &self,
        namespaced_name: &str,
    ) -> super::Result<(String, Arc<RunningService<RoleClient, McpClient>>)> {
        let (server, prompt) = split_on_server_name(namespaced_name)
            .ok_or_else(|| McpError::InvalidToolNameFormat(namespaced_name.to_string()))?;
        let client = self.clients.get(server).cloned().ok_or_else(|| McpError::ServerNotFound(server.to_string()))?;
        Ok((prompt.to_string(), client))
    }
}
