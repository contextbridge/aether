use crate::composer::attachment::{AttachmentOutcome, PromptAttachment};
use crate::request::RequestId;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::conversation::ConversationId;
use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewResponse, WorkspaceListResponse, WorkspaceMoveResponse,
    WorkspaceMoveTarget,
};
use agent_client_protocol::schema::v2::{
    ContentBlock, ListSessionsResponse, LoginAuthResponse, NewSessionResponse, ResumeSessionResponse,
    SessionConfigOptionValue, SessionId, SetSessionConfigOptionResponse,
};
use clankerdiff_client::protocol::server::ServerMessage;
use clankerdiff_client::{ClientState, DiffReviewEvent};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

/// Work a host asks the outside world to do. `T` carries the host's own
/// commands, which its [`HostExecutor`](crate::runtime::HostExecutor) runs.
#[derive(Debug, Clone)]
pub enum Command<T> {
    Agent(AgentCommand),
    GitReview(GitReviewCommand),
    Filesystem(FilesystemCommand),
    Host(T),
}

impl Command<Infallible> {
    /// Widens a command without a host part into any host's command type.
    pub fn widen<T>(self) -> Command<T> {
        match self {
            Self::Agent(command) => Command::Agent(command),
            Self::GitReview(command) => Command::GitReview(command),
            Self::Filesystem(command) => Command::Filesystem(command),
            Self::Host(never) => match never {},
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt {
        request_id: RequestId,
        session_id: SessionId,
        text: String,
        content: Option<Vec<ContentBlock>>,
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
        session_id: String,
    },
    ListWorkspaces {
        session_id: String,
    },
    MoveWorkspace {
        session_id: String,
        target: WorkspaceMoveTarget,
    },
    FetchWorkspaceStatus {
        session_id: String,
        cwd: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub enum GitReviewCommand {
    /// Starts a review client for the session, opening the agent-side server.
    Open { session_id: String },
    /// Applies one user action the review screen captured.
    Event(DiffReviewEvent),
    /// Feeds one agent-side live-protocol message into the review client.
    Forward(ServerMessage),
    /// Stops the review and tears down the agent-side server.
    Close,
}

#[derive(Debug, Clone)]
pub enum FilesystemCommand {
    IndexFiles { request_id: RequestId, root: PathBuf },
    PrepareSubmission { attachments: Vec<PromptAttachment> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRejection {
    Cancelled,
    Failed(String),
}

/// The outcome of a [`Command`]. `T` carries the results of host commands.
pub enum CommandResult<T> {
    Prompt { request_id: RequestId, result: Result<(), PromptRejection> },
    Cancel(Result<(), String>),
    AuthenticateMcp(Result<(), String>),
    ConfigOptionsUpdated { conversation_id: ConversationId, result: Result<SetSessionConfigOptionResponse, String> },
    AuthenticationCompleted { method_id: String, result: Result<LoginAuthResponse, String> },
    NewSession(Result<NewSessionResponse, String>),
    ResumeSession { session_id: SessionId, result: Result<ResumeSessionResponse, String> },
    SessionsListed(Result<ListSessionsResponse, String>),
    PromptSearchResults { query: String, result: Result<PromptSearchResponse, String> },
    SessionPreviewLoaded { session_id: String, result: Result<SessionPreviewResponse, String> },
    WorkspacesListed(Result<WorkspaceListResponse, String>),
    WorkspaceMoved(Result<WorkspaceMoveResponse, String>),
    FilesIndexed { request_id: RequestId, files: Vec<PromptAttachment> },
    GitReview(Arc<ClientState>),
    GitReviewAction(Result<(), String>),
    SubmissionPrepared(AttachmentOutcome),
    WorkspaceResolved { cwd: PathBuf, status: WorkspaceStatus },
    BackgroundFailed(String),
    Host(T),
}
