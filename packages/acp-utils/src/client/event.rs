use acp::Error;
use acp::Responder;
use acp::schema::{SessionUpdate, StopReason};
use agent_client_protocol as acp;
use agent_client_protocol::schema::{SessionConfigOption, SessionId, SessionInfo};

use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextUsageParams, ElicitationParams, ElicitationResponse,
    McpNotification, PromptSearchResponse, SubAgentProgressParams,
};

/// Events forwarded from the ACP connection to the main event loop.
pub enum AcpEvent {
    SessionUpdate { session_id: SessionId, update: Box<SessionUpdate> },
    ContextCleared(ContextClearedParams),
    ContextUsage(ContextUsageParams),
    SubAgentProgress(SubAgentProgressParams),
    AuthMethodsUpdated(AuthMethodsUpdatedParams),
    McpNotification(McpNotification),
    ElicitationRequest { params: ElicitationParams, responder: Responder<ElicitationResponse> },
    PromptDone(StopReason),
    PromptError(Error),
    AuthenticateComplete { method_id: String },
    AuthenticateFailed { method_id: String, error: String },
    SessionsListed { sessions: Vec<SessionInfo> },
    SessionLoaded { session_id: SessionId, config_options: Vec<SessionConfigOption> },
    NewSessionCreated { session_id: SessionId, config_options: Vec<SessionConfigOption> },
    PromptSearchResults(PromptSearchResponse),
    PromptSearchFailed { query: String, error: String },
    ConnectionClosed,
}
