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

impl ThreadStatus {
    /// Whether the agent connection failed or closed on its own, which leaves the thread waiting
    /// for the user to reconnect it.
    pub fn can_reconnect(&self) -> bool {
        matches!(self, Self::Errored(_) | Self::Disconnected)
    }
}
