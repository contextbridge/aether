use acp::Responder;
use acp::schema::v2::{SessionUpdate, StopReason};
use agent_client_protocol as acp;
use agent_client_protocol::schema::v2::{
    CreateElicitationRequest, CreateElicitationResponse, SessionId, UpdateSessionNotification,
};

use crate::client::ResumedSession;
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, SessionUsageParams,
    SubAgentProgressParams,
};

pub enum ReplayableEvent {
    SessionUpdate(Box<UpdateSessionNotification>),
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(Box<SubAgentProgressParams>),
    SessionUsage(Box<SessionUsageParams>),
    McpNotification(McpNotification),
}

impl From<UpdateSessionNotification> for ReplayableEvent {
    fn from(notification: UpdateSessionNotification) -> Self {
        Self::SessionUpdate(Box::new(notification))
    }
}

impl From<ReplayableEvent> for AcpEvent {
    fn from(event: ReplayableEvent) -> Self {
        match event {
            ReplayableEvent::SessionUpdate(notification) => {
                Self::SessionUpdate { session_id: notification.session_id, update: Box::new(notification.update) }
            }
            ReplayableEvent::ContextCleared(params) => Self::ContextCleared(params),
            ReplayableEvent::ContextCompaction(params) => Self::ContextCompaction(params),
            ReplayableEvent::SubAgentProgress(params) => Self::SubAgentProgress(*params),
            ReplayableEvent::SessionUsage(params) => Self::SessionUsage(params),
            ReplayableEvent::McpNotification(params) => Self::McpNotification(params),
        }
    }
}

/// Events forwarded from the ACP connection to the main event loop.
pub enum AcpEvent {
    SessionResumed(ResumedSession),
    SessionUpdate { session_id: SessionId, update: Box<SessionUpdate> },
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(SubAgentProgressParams),
    SessionUsage(Box<SessionUsageParams>),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    ElicitationRequest { params: Box<CreateElicitationRequest>, responder: Responder<CreateElicitationResponse> },
    PromptCompleted { session_id: SessionId, stop_reason: StopReason },
    ConnectionClosed,
}
