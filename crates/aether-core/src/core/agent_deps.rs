use crate::core::AgentRegistry;
use crate::events::{AgentObserver, DynObserverFactory, TraceContext};
use aether_auth::OAuthCredentialStorage;
use mcp_utils::model::ElicitationCapability;
use std::sync::Arc;

/// Cross-cutting dependencies threaded to every agent a run spawns — the root
/// agent and any sub-agents created by in-memory MCP servers. Bundling them
/// keeps the plumbing through builders and servers a single value.
#[derive(Clone, Default)]
pub struct AgentDeps {
    pub oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    pub observer_factory: Option<DynObserverFactory>,
    /// Remote trace these agents continue, set by whoever handled the request
    /// that spawned them.
    pub parent_trace_context: Option<TraceContext>,
    pub agent_registry: AgentRegistry,
    pub mcp_elicitation: Option<ElicitationCapability>,
    pub session_affinity_key: Option<String>,
}

impl AgentDeps {
    pub fn new(
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
        observer_factory: Option<DynObserverFactory>,
    ) -> Self {
        Self { oauth_credential_store: Some(oauth_credential_store), observer_factory, ..Self::default() }
    }

    /// Continue `parent`'s trace in every agent built from these deps.
    pub fn with_parent_trace_context(mut self, parent: Option<TraceContext>) -> Self {
        self.parent_trace_context = parent;
        self
    }

    pub fn with_agent_registry(mut self, registry: AgentRegistry) -> Self {
        self.agent_registry = registry;
        self
    }

    pub fn with_mcp_elicitation(mut self, capability: Option<ElicitationCapability>) -> Self {
        self.mcp_elicitation = capability;
        self
    }

    pub fn with_session_affinity_key(mut self, key: impl Into<String>) -> Self {
        self.session_affinity_key = Some(key.into());
        self
    }

    /// A fresh observer isolated to one agent, if a factory is configured.
    pub fn observer(&self, agent_name: &str) -> Option<Box<dyn AgentObserver>> {
        self.observer_factory
            .as_ref()
            .map(|factory| factory.agent(Some(agent_name), self.parent_trace_context.as_ref()))
    }
}
