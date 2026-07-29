use crate::agent_spec::AgentSpec;
use std::sync::Arc;
use thiserror::Error;

/// A cheap-to-clone registry of resolved agent specifications.
#[derive(Clone, Debug, Default)]
pub struct AgentRegistry {
    specs: Arc<[AgentSpec]>,
}

impl AgentRegistry {
    pub fn new(specs: Vec<AgentSpec>) -> Self {
        Self { specs: specs.into() }
    }

    pub fn all(&self) -> &[AgentSpec] {
        &self.specs
    }

    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.specs.iter().find(|spec| spec.name == name)
    }

    pub fn agent_invocable(&self) -> impl Iterator<Item = &AgentSpec> {
        self.specs.iter().filter(|spec| spec.exposure.agent_invocable)
    }

    /// Resolve an agent that is exposed for delegation.
    pub fn resolve_agent_invocable(&self, name: &str) -> Result<AgentSpec, AgentRegistryError> {
        let spec = self.get(name).ok_or_else(|| AgentRegistryError::NotFound { name: name.to_string() })?;
        if !spec.exposure.agent_invocable {
            return Err(AgentRegistryError::NotAgentInvocable { name: name.to_string() });
        }
        Ok(spec.clone())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AgentRegistryError {
    #[error("Agent '{name}' not found")]
    NotFound { name: String },
    #[error("Agent '{name}' is not agent-invocable")]
    NotAgentInvocable { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_spec::AgentSpecExposure;

    #[test]
    fn delegation_resolution_distinguishes_missing_and_unexposed_agents() {
        let model = "anthropic:claude-sonnet-4-5".parse().unwrap();
        let mut delegate = AgentSpec::bare(&model, None, Vec::new());
        delegate.name = "delegate".to_string();
        delegate.exposure = AgentSpecExposure::agent_only();
        let mut user_only = AgentSpec::bare(&model, None, Vec::new());
        user_only.name = "user-only".to_string();
        user_only.exposure = AgentSpecExposure::user_only();
        let registry = AgentRegistry::new(vec![delegate, user_only]);

        assert_eq!(registry.resolve_agent_invocable("delegate").unwrap().name, "delegate");
        assert!(matches!(
            registry.resolve_agent_invocable("user-only"),
            Err(AgentRegistryError::NotAgentInvocable { name }) if name == "user-only"
        ));
        assert!(matches!(
            registry.resolve_agent_invocable("missing"),
            Err(AgentRegistryError::NotFound { name }) if name == "missing"
        ));
    }
}
