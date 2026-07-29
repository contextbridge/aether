use crate::core::AgentRegistry;
use crate::events::{AgentObserver, DynObserverFactory, TraceContext};
use aether_auth::OAuthCredentialStorage;
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
}

impl AgentDeps {
    pub fn new(
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
        observer_factory: Option<DynObserverFactory>,
    ) -> Self {
        Self {
            oauth_credential_store: Some(oauth_credential_store),
            observer_factory,
            parent_trace_context: None,
            agent_registry: AgentRegistry::default(),
        }
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

    /// A fresh observer isolated to one agent, if a factory is configured.
    pub fn observer(&self) -> Option<Box<dyn AgentObserver>> {
        self.observer_factory.as_ref().map(|factory| factory.agent(self.parent_trace_context.as_ref()))
    }
}
