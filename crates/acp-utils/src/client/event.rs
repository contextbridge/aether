use acp::Responder;
use acp::schema::v1::{SessionUpdate, StopReason};
use agent_client_protocol as acp;
use agent_client_protocol::schema::v1::{CreateElicitationRequest, CreateElicitationResponse, SessionId, SessionInfo};

use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, SubAgentProgressParams,
};

/// Events forwarded from the ACP connection to the main event loop.
pub enum AcpEvent {
    SessionUpdate { session_id: SessionId, update: Box<SessionUpdate> },
    ContextCleared(ContextClearedParams),
    ContextCompaction(ContextCompactionParams),
    SubAgentProgress(SubAgentProgressParams),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    ElicitationRequest { params: Box<CreateElicitationRequest>, responder: Responder<CreateElicitationResponse> },
    PromptDone(StopReason),
    PromptError(acp::Error),
    SessionsListed { sessions: Vec<SessionInfo> },
    ConnectionClosed,
}
