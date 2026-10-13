use super::deferred_tools;
use super::tool_bridge::tool_definition;
use super::{ToolExposure, ToolFilter};
use crate::core::McpInstructions;
use futures::Stream;
use llm::ToolDefinition;
use mcp_utils::client::{ClientOptions, ToolCall, ToolCallOptions};
use mcp_utils::gateway::{ConnectedServer, GatewayError, McpGateway, ServerPrompt};
use mcp_utils::model::{GetPromptResult, Tool};
use mcp_utils::server::{ServerHandle, UnixSocket};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use utils::mcp_status::McpServerStatusEntry;

#[derive(Clone)]
pub struct McpCatalog {
    core: CatalogCore,
    deferred_tools_server: Option<Arc<ServerHandle>>,
}

impl McpCatalog {
    pub async fn ready(&self) {
        self.core.gateway.ready().await;
    }

    pub fn changes(&self) -> impl Stream<Item = ()> + Send + Sync + Unpin + use<> {
        self.core.gateway.changes()
    }

    pub fn statuses(&self) -> Vec<McpServerStatusEntry> {
        self.core.gateway.statuses().into_iter().map(|status| self.core.policy.status_entry(status)).collect()
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.core.tools(Tier::Direct).into_iter();
        tools.map(|(server, tool)| tool_definition(&tool).with_server(server)).collect()
    }

    pub fn instructions(&self) -> McpInstructions {
        let servers = self.core.servers(Tier::Direct).into_iter();
        McpInstructions {
            servers: servers
                .filter_map(|(server, _)| Some((server.name().to_string(), server.instructions()?)))
                .collect(),
            progressive_discovery: !self.core.servers(Tier::Deferred).is_empty(),
        }
    }

    pub fn call_tool(&self, name: &str, args: Map<String, Value>, options: ToolCallOptions) -> Option<ToolCall> {
        self.core.call(Tier::Direct, name, args, options)
    }

    pub async fn list_prompts(&self) -> Vec<ServerPrompt> {
        self.core.gateway.list_prompts().await
    }

    pub async fn get_prompt(
        &self,
        server: &str,
        prompt: &str,
        args: Map<String, Value>,
    ) -> Result<GetPromptResult, GatewayError> {
        self.core.gateway.get_prompt(server, prompt, args).await
    }

    pub async fn authenticate(&self, server: &str) -> Result<(), GatewayError> {
        self.core.gateway.authenticate(server).await
    }

    pub fn deferred_tools_socket(&self) -> Option<&Path> {
        self.deferred_tools_server.as_deref().map(ServerHandle::path)
    }

    pub async fn shutdown(&self) {
        if let Some(server) = &self.deferred_tools_server {
            server.stop();
        }
        self.core.gateway.shutdown().await;
    }

    pub(crate) fn new(
        gateway: McpGateway,
        filter: ToolFilter,
        exposures: HashMap<String, ToolExposure>,
        socket: Option<UnixSocket>,
    ) -> Self {
        let core = CatalogCore { gateway, policy: Arc::new(ToolPolicy { filter, exposures }) };
        let deferred_tools_server = socket.map(|socket| Arc::new(deferred_tools::serve(core.clone(), socket)));
        Self { core, deferred_tools_server }
    }
}

impl Default for McpCatalog {
    fn default() -> Self {
        let gateway = McpGateway::new(Vec::new(), &ClientOptions::default());
        Self::new(gateway, ToolFilter::default(), HashMap::new(), None)
    }
}

pub(super) fn namespaced(server: &str, tool: &str) -> String {
    format!("{server}{SERVER_DELIMITER}{tool}")
}

const SERVER_DELIMITER: &str = "__";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tier {
    Direct,
    Deferred,
}

#[derive(Clone)]
pub(super) struct CatalogCore {
    gateway: McpGateway,
    policy: Arc<ToolPolicy>,
}

impl CatalogCore {
    pub(super) fn servers(&self, tier: Tier) -> Vec<(ConnectedServer, Vec<Tool>)> {
        let servers = self.gateway.servers().into_iter();
        servers
            .filter_map(|server| {
                let offered = server.tools().iter().filter(|tool| self.policy.tier(server.name(), tool) == Some(tier));
                let offered = offered.cloned().collect::<Vec<_>>();
                (!offered.is_empty()).then_some((server, offered))
            })
            .collect()
    }

    pub(super) fn tools(&self, tier: Tier) -> Vec<(String, Tool)> {
        let servers = self.servers(tier).into_iter();
        servers
            .flat_map(|(server, tools)| {
                let server = server.name().to_string();
                tools.into_iter().map(move |mut tool| {
                    tool.name = namespaced(&server, &tool.name).into();
                    (server.clone(), tool)
                })
            })
            .collect()
    }

    pub(super) fn tier_of(&self, name: &str) -> Option<Tier> {
        self.resolve(name).map(|(_, _, tier)| tier)
    }

    pub(super) fn call(
        &self,
        tier: Tier,
        name: &str,
        args: Map<String, Value>,
        options: ToolCallOptions,
    ) -> Option<ToolCall> {
        let (server, tool, offered_in) = self.resolve(name)?;
        (offered_in == tier).then(|| server.call_tool(tool, args, options))
    }

    fn resolve<'n>(&self, name: &'n str) -> Option<(ConnectedServer, &'n str, Tier)> {
        let (server, tool) = self.split(name)?;
        let server = self.gateway.server(server)?;
        let tier = self.policy.tier(server.name(), server.tools().iter().find(|offered| offered.name == tool)?)?;
        Some((server, tool, tier))
    }

    fn split<'n>(&self, name: &'n str) -> Option<(&str, &'n str)> {
        let servers = self.policy.exposures.keys();
        servers
            .filter_map(|server| Some((server.as_str(), name.strip_prefix(server)?.strip_prefix(SERVER_DELIMITER)?)))
            .max_by_key(|(server, _)| server.len())
    }
}

struct ToolPolicy {
    filter: ToolFilter,
    exposures: HashMap<String, ToolExposure>,
}

impl ToolPolicy {
    fn tier(&self, server: &str, tool: &Tool) -> Option<Tier> {
        if !self.filter.allows(&namespaced(server, &tool.name), tool) {
            return None;
        }
        let deferred = self.exposures.get(server).is_some_and(|exposure| exposure.is_deferred(tool));
        Some(if deferred { Tier::Deferred } else { Tier::Direct })
    }

    fn status_entry(&self, status: McpServerStatusEntry) -> McpServerStatusEntry {
        let defers_tools = self.exposures.get(&status.name).is_some_and(ToolExposure::defers_tools);
        status.with_deferred_tools(defers_tools)
    }
}
