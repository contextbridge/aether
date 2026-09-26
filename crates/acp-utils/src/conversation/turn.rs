use agent_client_protocol::schema::v2 as acp;
use schemars::JsonSchema;
use serde::Serialize;
use thiserror::Error;

/// Where the current prompt is in its lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// No prompt is outstanding; a new one can be sent.
    #[default]
    Idle,
    /// A prompt was sent and the agent has not accepted it yet.
    Submitting,
    /// The agent accepted the prompt, or started a turn itself, and has not reached idle.
    Running,
    /// The agent reached idle before accepting the prompt, whose response is still owed.
    CompletedBeforeAcceptance,
}

/// A turn the conversation was waiting on reached idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFinished {
    pub stop_reason: Option<acp::StopReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the session is already in a turn")]
pub struct TurnInProgress;

impl TurnPhase {
    pub fn is_idle(self) -> bool {
        self == Self::Idle
    }

    pub fn waiting_for_response(self) -> bool {
        matches!(self, Self::Submitting | Self::Running)
    }

    pub(super) fn accepted(self) -> Self {
        match self {
            Self::Submitting => Self::Running,
            Self::CompletedBeforeAcceptance => Self::Idle,
            Self::Idle | Self::Running => self,
        }
    }

    pub(super) fn finished(self) -> Self {
        match self {
            Self::Submitting => Self::CompletedBeforeAcceptance,
            Self::Running => Self::Idle,
            Self::Idle | Self::CompletedBeforeAcceptance => self,
        }
    }
}
