#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Connecting,
    Idle,
    Running,
    Background,
    NeedsInput,
    Errored,
    Disconnected,
    Dormant,
}

impl ThreadStatus {
    pub fn is_busy(self) -> bool {
        matches!(self, Self::Running | Self::Background | Self::NeedsInput)
    }

    pub fn has_agent(self) -> bool {
        matches!(self, Self::Connecting | Self::Idle | Self::Running | Self::Background | Self::NeedsInput)
    }

    pub fn problem(self) -> Option<&'static str> {
        match self {
            Self::Errored => Some("The agent could not start"),
            Self::Disconnected => Some("The agent connection closed"),
            _ => None,
        }
    }

    pub fn can_reconnect(self) -> bool {
        self.problem().is_some()
    }
}
