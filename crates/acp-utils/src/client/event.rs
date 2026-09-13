use crate::client::ResumedSession;
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, SubAgentProgressParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::v2::{
    CreateElicitationRequest, CreateElicitationResponse, UpdateSessionNotification,
};

/// Events forwarded from the ACP connection to the main event loop.
pub enum AcpEvent {
    SessionResumed(ResumedSession),
    SessionUpdate(Box<UpdateSessionNotification>),
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(SubAgentProgressParams),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    ElicitationRequest { params: Box<CreateElicitationRequest>, responder: Responder<CreateElicitationResponse> },
    ConnectionClosed,
}

impl From<UpdateSessionNotification> for AcpEvent {
    fn from(notification: UpdateSessionNotification) -> Self {
        Self::SessionUpdate(Box::new(notification))
    }
}
