use client_core::session_core::{Elicitation, SessionCore};

/// What a thread is doing, as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadStatus {
    /// The agent is starting and its session opening.
    Connecting,
    /// The agent waits for a prompt.
    Idle,
    /// A prompt or one of its tool calls is still running.
    Running,
    /// The agent asked the user something and waits for the answer.
    NeedsInput,
    /// The agent could not be reached.
    Errored(String),
    /// The agent connection closed on its own.
    Disconnected,
    /// No agent runs for the thread: it was stopped, or is only in history.
    Dormant,
}

/// How far a thread's agent connection has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionPhase {
    Connecting,
    Connected,
    /// Connecting failed, for the given reason.
    Failed(String),
    /// The connection closed on its own.
    Closed,
    /// The user stopped the thread, ending its agent.
    Stopped,
}

impl ThreadStatus {
    /// Derives a thread's status from its connection, its session state, and the elicitation it
    /// is waiting on, if any.
    pub fn derive(phase: &ConnectionPhase, core: Option<&SessionCore>, elicitation: Option<&Elicitation>) -> Self {
        match phase {
            ConnectionPhase::Connecting => Self::Connecting,
            ConnectionPhase::Failed(error) => Self::Errored(error.clone()),
            ConnectionPhase::Closed => Self::Disconnected,
            ConnectionPhase::Stopped => Self::Dormant,
            ConnectionPhase::Connected if elicitation.is_some() => Self::NeedsInput,
            ConnectionPhase::Connected if core.is_some_and(SessionCore::is_agent_busy) => Self::Running,
            ConnectionPhase::Connected => Self::Idle,
        }
    }

    /// Whether the agent connection failed or closed on its own, which leaves the thread waiting
    /// for the user to reconnect it.
    pub fn can_reconnect(&self) -> bool {
        matches!(self, Self::Errored(_) | Self::Disconnected)
    }
}
