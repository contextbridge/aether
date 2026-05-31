#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AgentKey {
    Default,
    Named(String),
}

impl AgentKey {
    /// The mode name backing this key, or `None` for the unnamed default agent.
    pub(crate) fn agent_name(&self) -> Option<String> {
        match self {
            Self::Default => None,
            Self::Named(name) => Some(name.clone()),
        }
    }

    /// Human-readable name for logs and error messages.
    pub(crate) fn display_name(&self) -> String {
        self.agent_name().unwrap_or_else(|| "default".to_string())
    }
}
