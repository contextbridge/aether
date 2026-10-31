use crate::composer::attachment::{AttachmentOutcome, PromptAttachment};
use crate::session_model::{WorkspaceAccess, WorkspaceStatus};
use acp_utils::client::AcpClientError;
use acp_utils::conversation::ConversationId;
use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewResponse, WorkspaceListResponse, WorkspaceMoveResponse,
    WorkspaceMoveTarget,
};
use agent_client_protocol::schema::v2::{
    ContentBlock, ListSessionsResponse, LoginAuthResponse, NewSessionResponse, ResumeSessionResponse,
    SessionConfigOptionValue, SessionId, SessionInfo, SetSessionConfigOptionResponse,
};
use clankerdiff_client::{ClientError, ClientState, ConnectionHeader, DiffReviewEvent, LookupReply};
use clankerdiff_remote::Invocation;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task::JoinError;

#[derive(Debug, Clone)]
pub enum Command {
    Agent(AgentCommand),
    GitReview(GitReviewCommand),
    PrepareSubmission { attachments: Vec<PromptAttachment>, access: WorkspaceAccess },
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt {
        request_id: RequestId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
    },
    Cancel {
        session_id: SessionId,
    },
    SetConfigOption {
        conversation_id: ConversationId,
        session_id: SessionId,
        config_id: String,
        value: SessionConfigOptionValue,
    },
    AuthenticateMcpServer {
        session_id: SessionId,
        server_name: String,
    },
    Authenticate {
        method_id: String,
    },
    ListSessions,
    ResumeSession {
        session_id: SessionId,
        cwd: PathBuf,
    },
    NewSession {
        cwd: PathBuf,
    },
    SearchPrompts(PromptSearchParams),
    SessionPreview {
        session_id: SessionId,
    },
    ListWorkspaces {
        session_id: SessionId,
    },
    MoveWorkspace {
        session_id: SessionId,
        target: WorkspaceMoveTarget,
    },
    FetchWorkspaceStatus {
        session_id: SessionId,
        cwd: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub enum GitReviewCommand {
    Open(ReviewSource),
    Event(DiffReviewEvent),
    Close,
}

#[derive(Debug, Clone)]
pub enum ReviewSource {
    Agent { session_id: SessionId },
    Local(PathBuf),
    Stdio(Invocation),
    WebSocket { url: String, headers: Vec<ConnectionHeader> },
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRejection {
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub enum CommandResult {
    Prompt {
        request_id: RequestId,
        result: Result<(), PromptRejection>,
    },
    Notified(Result<(), AcpClientError>),
    ConfigOptionsUpdated {
        conversation_id: ConversationId,
        result: Result<SetSessionConfigOptionResponse, AcpClientError>,
    },
    NewSession(Result<NewSessionResponse, AcpClientError>),
    ResumeSession {
        session_id: SessionId,
        result: Result<ResumeSessionResponse, AcpClientError>,
    },
    SessionsListed(Result<ListSessionsResponse, AcpClientError>),
    PromptSearchResults {
        query: String,
        result: Result<PromptSearchResponse, AcpClientError>,
    },
    WorkspacesListed(Result<WorkspaceListResponse, AcpClientError>),
    WorkspaceMoved(Result<WorkspaceMoveResponse, AcpClientError>),
    GitReview(Arc<ClientState>),
    SubmissionPrepared(AttachmentOutcome),
    WorkspaceResolved {
        cwd: PathBuf,
        status: WorkspaceStatus,
    },
    BackgroundFailed(JoinError),
    Reply(Reply),
}

#[derive(Debug)]
pub enum Reply {
    SessionsListed(Vec<SessionInfo>),
    SessionPreviewLoaded { session_id: SessionId, result: Result<SessionPreviewResponse, AcpClientError> },
    AuthenticationCompleted { method_id: String, result: Result<LoginAuthResponse, AcpClientError> },
    GitReviewAction(Result<(), ClientError>),
    GitReviewLookup(LookupReply),
}

impl From<JoinError> for CommandResult {
    fn from(error: JoinError) -> Self {
        Self::BackgroundFailed(error)
    }
}

impl RequestId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
