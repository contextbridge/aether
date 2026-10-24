use super::ToolStatus;
use agent_client_protocol::schema::v2 as acp;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    #[default]
    Idle,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFinished {
    pub stop_reason: Option<acp::StopReason>,
}

impl TurnPhase {
    pub fn is_idle(self) -> bool {
        self == Self::Idle
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum WorkState {
    #[default]
    Unconfirmed,
    Running,
    Idle(ToolStatus),
}

impl WorkState {
    pub(super) fn phase(self) -> TurnPhase {
        match self {
            Self::Running => TurnPhase::Running,
            Self::Unconfirmed | Self::Idle(_) => TurnPhase::Idle,
        }
    }

    pub(super) fn status(self) -> Option<ToolStatus> {
        match self {
            Self::Unconfirmed => None,
            Self::Running => Some(ToolStatus::Running),
            Self::Idle(status) => Some(status),
        }
    }

    pub(super) fn is_idle(self) -> bool {
        matches!(self, Self::Idle(_))
    }
}
