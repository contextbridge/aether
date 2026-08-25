use aether_core::agent_spec::AgentSpec;
use aether_project::AgentCatalog;

use super::agent_key::AgentKey;

#[derive(Clone)]
pub(crate) struct SessionAgents {
    catalog: AgentCatalog,
    default_spec: Option<AgentSpec>,
}

impl SessionAgents {
    pub(crate) fn new(catalog: AgentCatalog) -> Self {
        Self { catalog, default_spec: None }
    }

    pub(crate) fn catalog(&self) -> &AgentCatalog {
        &self.catalog
    }

    pub(crate) fn set_default(&mut self, spec: AgentSpec) {
        self.default_spec = Some(spec);
    }

    pub(crate) fn get(&self, key: &AgentKey) -> Option<&AgentSpec> {
        match key {
            AgentKey::Default => self.default_spec.as_ref(),
            AgentKey::Named(name) => self.catalog.get(name).ok().filter(|spec| spec.exposure.user_invocable),
        }
    }
}
