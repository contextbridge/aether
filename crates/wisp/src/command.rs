use crate::attachment::{AttachmentOutcome, PromptAttachment};
use crate::conversation::ConversationId;
use crate::file_index::FileEntry;
use crate::git_review::{ClientState, DiffReviewEvent, ServerMessage};
use crate::request::RequestId;
use crate::session::workspace_status::WorkspaceStatus;
use crate::settings::UiSettings;
use crate::theme::{Theme, ThemeApplicationError};
use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewResponse, WorkspaceListResponse, WorkspaceMoveResponse,
    WorkspaceMoveTarget,
};
use agent_client_protocol::schema::v2::{
    ContentBlock, ListSessionsResponse, LoginAuthResponse, NewSessionResponse, PromptResponse, ResumeSessionResponse,
    SessionConfigOptionValue, SessionId, SetSessionConfigOptionResponse,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Command {
    Agent(AgentCommand),
    GitReview(GitReviewCommand),
    Filesystem(FilesystemCommand),
    Terminal(TerminalCommand),
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt {
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
    ListThemes,
    ListReviewThemes,
    ApplyTheme { settings: Box<UiSettings> },
}

#[derive(Debug, Clone)]
pub enum TerminalCommand {
    RingBell,
}

pub enum CommandResult {
    Prompt(Result<PromptResponse, String>),
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
    FilesIndexed { request_id: RequestId, files: Vec<FileEntry> },
    GitReview(Arc<ClientState>),
    GitReviewAction(Result<(), String>),
    SubmissionPrepared(AttachmentOutcome),
    ThemesListed(Vec<String>),
    ReviewThemesListed(Vec<clankerdiff_ratatui::ThemeChoice>),
    ThemeApplied(Result<(Box<UiSettings>, Theme), ThemeApplicationError>),
    WorkspaceResolved { cwd: PathBuf, status: WorkspaceStatus },
    BackgroundFailed(String),
    TerminalFailed(String),
}
