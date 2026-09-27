use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, GitDiffEventPayload, McpNotification, SubAgentProgressParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::v2::{
    CreateElicitationRequest, CreateElicitationResponse, ElicitationScope, SessionId, UpdateSessionNotification,
};

/// Events forwarded from the ACP connection to the main event loop.
pub enum AcpEvent {
    SessionUpdate(Box<UpdateSessionNotification>),
    ContextCleared(ContextClearedParams),
    SubAgentProgress(SubAgentProgressParams),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    GitDiffEvent(GitDiffEventPayload),
    ElicitationRequest { params: Box<CreateElicitationRequest>, responder: Responder<CreateElicitationResponse> },
    ConnectionClosed,
}

impl AcpEvent {
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::SessionUpdate(notification) => Some(&notification.session_id),
            Self::ContextCleared(params) => Some(&params.session_id),
            Self::SubAgentProgress(params) => Some(&params.session_id),
            Self::GitDiffEvent(params) => Some(&params.session_id),
            Self::ElicitationRequest { params, .. } => match params.scope() {
                ElicitationScope::Session(scope) => Some(&scope.session_id),
                _ => None,
            },
            Self::AuthMethodsUpdated(_) | Self::McpNotification(_) | Self::ConnectionClosed => None,
        }
    }
}

impl From<UpdateSessionNotification> for AcpEvent {
    fn from(notification: UpdateSessionNotification) -> Self {
        Self::SessionUpdate(Box::new(notification))
    }
}
