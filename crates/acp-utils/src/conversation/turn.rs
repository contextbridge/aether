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
