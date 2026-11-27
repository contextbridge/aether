use super::deferred_tools;
use super::skills_host::SkillsHost;
use super::tool_bridge::tool_definition;
use super::{ToolExposure, ToolFilter};
use crate::core::McpInstructions;
use foxfire_acp::mcp_status::McpServerStatusEntry;
use futures::{Stream, stream};
use llm::ToolDefinition;
use mcp_utils::client::{ClientOptions, ToolCall, ToolCallOptions};
use mcp_utils::gateway::{ConnectedServer, GatewayError, McpGateway, ServerPrompt, ServerSpec};
use mcp_utils::model::{GetPromptResult, Tool};
use mcp_utils::server::{ServerHandle, UnixSocket};
use mcp_utils::tool_kind::{ToolKind, declared_tool_kind};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct McpCatalog {
    core: CatalogCore,
    deferred_tools_server: Option<Arc<ServerHandle>>,
}

impl McpCatalog {
    pub async fn ready(&self) {
        self.core.gateway.ready().await;
        self.core.skills_host.ready().await;
    }

    pub fn changes(&self) -> impl Stream<Item = ()> + Send + Sync + Unpin + use<> {
        stream::select(self.core.gateway.changes(), self.core.skills_host.changes())
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

    pub(crate) fn tool_kind(&self, name: &str) -> Option<ToolKind> {
        self.core.with_resolved_tool(name, |_, tool, tier| match tier {
            Tier::Direct => declared_tool_kind(tool),
            Tier::Deferred => None,
        })
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
        self.core.skills_host.shutdown().await;
        self.core.gateway.shutdown().await;
    }

    pub(crate) fn new(
        gateway: McpGateway,
        filter: ToolFilter,
        mut exposures: HashMap<String, ToolExposure>,
        socket: Option<UnixSocket>,
    ) -> Self {
        let skills_host = SkillsHost::new("aether", gateway.clone());
        exposures.entry(skills_host.name().to_string()).or_default();
        let skills_host = ServerSpec::in_process(skills_host.name().to_string(), skills_host);
        let skills_host = if gateway.statuses().is_empty() { Vec::new() } else { vec![skills_host] };
        let skills_host = McpGateway::new(skills_host, &ClientOptions::default());
        let core = CatalogCore { gateway, skills_host, policy: Arc::new(ToolPolicy { filter, exposures }) };
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
    skills_host: McpGateway,
    policy: Arc<ToolPolicy>,
}

impl CatalogCore {
    pub(super) fn servers(&self, tier: Tier) -> Vec<(ConnectedServer, Vec<Tool>)> {
        let servers = self.connected().into_iter();
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
        self.with_resolved_tool(name, |_, _, tier| Some(tier))
    }

    pub(super) fn call(
        &self,
        tier: Tier,
        name: &str,
        args: Map<String, Value>,
        options: ToolCallOptions,
    ) -> Option<ToolCall> {
        self.with_resolved_tool(name, |server, tool, offered_in| {
            (offered_in == tier).then(|| server.call_tool(&tool.name, args, options))
        })
    }

    fn with_resolved_tool<T>(
        &self,
        name: &str,
        with_tool: impl FnOnce(&ConnectedServer, &Tool, Tier) -> Option<T>,
    ) -> Option<T> {
        let (server, tool_name) = self.split(name)?;
        let server = self.connected().into_iter().find(|connected| connected.name() == server)?;
        let tool = server.tools().iter().find(|offered| offered.name == tool_name)?;
        let tier = self.policy.tier(server.name(), tool)?;
        with_tool(&server, tool, tier)
    }

    fn connected(&self) -> Vec<ConnectedServer> {
        let mut servers = self.gateway.servers();
        if servers.iter().any(|server| server.client().capabilities().supports_skills()) {
            servers.extend(self.skills_host.servers());
        }
        servers
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
