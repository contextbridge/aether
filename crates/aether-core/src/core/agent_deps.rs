use crate::core::AgentRegistry;
use crate::events::{AgentObserver, DynObserverFactory, TraceContext};
use aether_auth::OAuthCredentialStorage;
use rmcp::model::ClientCapabilities;
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
    pub mcp_client_capabilities: Option<ClientCapabilities>,
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

    pub fn with_mcp_client_capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.mcp_client_capabilities = Some(capabilities);
        self
    }

    pub fn supports_mcp_url_elicitation(&self) -> bool {
        self.mcp_client_capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.elicitation.as_ref())
            .is_some_and(|elicitation| elicitation.url.is_some())
    }

    /// A fresh observer isolated to one agent, if a factory is configured.
    pub fn observer(&self, agent_name: &str) -> Option<Box<dyn AgentObserver>> {
        self.observer_factory
            .as_ref()
            .map(|factory| factory.agent(Some(agent_name), self.parent_trace_context.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ElicitationCapability, FormElicitationCapability, UrlElicitationCapability};

    #[test]
    fn mcp_url_elicitation_support_requires_advertised_url_capability() {
        assert!(!AgentDeps::default().supports_mcp_url_elicitation());

        let mut form_only = ClientCapabilities::default();
        form_only.elicitation = Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
        assert!(!AgentDeps::default().with_mcp_client_capabilities(form_only).supports_mcp_url_elicitation());

        let mut url = ClientCapabilities::default();
        url.elicitation = Some(ElicitationCapability::new().with_url(UrlElicitationCapability::new()));
        assert!(AgentDeps::default().with_mcp_client_capabilities(url).supports_mcp_url_elicitation());
    }
}
