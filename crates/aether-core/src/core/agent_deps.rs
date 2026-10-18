use crate::core::AgentRegistry;
use crate::events::{AgentObserver, DynObserverFactory, TraceContext};
use aether_auth::{OAuthCredentialStorage, SigV4Signers};
use llm::CodexProvider;
use llm::parser::ModelProviderParser;
use std::sync::Arc;

/// Cross-cutting dependencies threaded to every agent a run spawns — the root
/// agent and any sub-agents created by in-memory MCP servers. Bundling them
/// keeps the plumbing through builders and servers a single value.
#[derive(Clone, Default)]
pub struct AgentDeps {
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    codex: Option<CodexProvider>,
    model_providers: Arc<ModelProviderParser>,
    pub sigv4_signers: SigV4Signers,
    pub observer_factory: Option<DynObserverFactory>,
    /// Remote trace these agents continue, set by whoever handled the request
    /// that spawned them.
    pub parent_trace_context: Option<TraceContext>,
    pub agent_registry: AgentRegistry,
    pub session_affinity_key: Option<String>,
}

impl AgentDeps {
    pub fn new(
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
        observer_factory: Option<DynObserverFactory>,
    ) -> Self {
        let codex = CodexProvider::new(Arc::clone(&oauth_credential_store));
        Self {
            oauth_credential_store: Some(oauth_credential_store),
            model_providers: Arc::new(ModelProviderParser::default().with_codex_provider(codex.clone())),
            codex: Some(codex),
            observer_factory,
            ..Self::default()
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

    pub fn with_session_affinity_key(mut self, key: impl Into<String>) -> Self {
        self.session_affinity_key = Some(key.into());
        self
    }

    pub fn oauth_credential_store(&self) -> Option<&Arc<dyn OAuthCredentialStorage>> {
        self.oauth_credential_store.as_ref()
    }

    pub fn model_providers(&self) -> &ModelProviderParser {
        &self.model_providers
    }

    pub async fn forget_cached_credentials(&self) {
        if let Some(codex) = &self.codex {
            codex.forget_cached_token().await;
        }
    }

    /// A fresh observer isolated to one agent, if a factory is configured.
    pub fn observer(&self, agent_name: &str) -> Option<Box<dyn AgentObserver>> {
        self.observer_factory
            .as_ref()
            .map(|factory| factory.agent(Some(agent_name), self.parent_trace_context.as_ref()))
    }
}
