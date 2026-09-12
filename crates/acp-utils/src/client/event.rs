use acp::Responder;
use acp::schema::v1::{SessionUpdate, StopReason};
use agent_client_protocol as acp;
use agent_client_protocol::schema::v1::{
    CreateElicitationRequest, CreateElicitationResponse, SessionId, SessionNotification,
};

use crate::client::LoadedSession;
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, SessionUsageParams,
    SubAgentProgressParams,
};

pub enum ReplayableEvent {
    SessionUpdate(Box<SessionNotification>),
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(Box<SubAgentProgressParams>),
    SessionUsage(Box<SessionUsageParams>),
    McpNotification(McpNotification),
}

impl From<SessionNotification> for ReplayableEvent {
    fn from(notification: SessionNotification) -> Self {
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
    SessionLoaded(LoadedSession),
    SessionUpdate { session_id: SessionId, update: Box<SessionUpdate> },
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(SubAgentProgressParams),
    SessionUsage(Box<SessionUsageParams>),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    ElicitationRequest { params: Box<CreateElicitationRequest>, responder: Responder<CreateElicitationResponse> },
    PromptCompleted(StopReason),
    ConnectionClosed,
}
