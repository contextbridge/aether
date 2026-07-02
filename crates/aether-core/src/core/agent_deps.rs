use crate::events::{AgentObserver, ObserverFactory};
use aether_auth::OAuthCredentialStorage;
use std::sync::Arc;

/// Cross-cutting dependencies threaded to every agent a run spawns — the root
/// agent and any sub-agents created by in-memory MCP servers. Bundling them
/// keeps the plumbing through builders and servers a single value
#[derive(Clone, Default)]
pub struct AgentDeps {
    pub oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    pub observer_factory: Option<ObserverFactory>,
}

impl AgentDeps {
    /// A fresh observer for one agent, if a factory is configured.
    pub fn observer(&self) -> Option<Box<dyn AgentObserver>> {
        self.observer_factory.as_ref().map(|factory| factory())
    }
}
