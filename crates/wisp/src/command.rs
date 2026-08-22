use crate::attachment::{AttachmentOutcome, PromptAttachment};
use crate::file_index::FileEntry;
use crate::git_review::{DiffScope, FileStatus, GitDiffEvent};
use crate::request::RequestId;
use crate::session::workspace_status::WorkspaceStatus;
use crate::settings::UiSettings;
use crate::theme::Theme;
use acp_utils::notifications::{PromptSearchParams, WorkspaceMoveTarget};
use agent_client_protocol::schema::v1::{ContentBlock, SessionId};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Command {
    Agent(AgentCommand),
    Filesystem(FilesystemCommand),
    Git(GitCommand),
    ResolveWorkspace { cwd: PathBuf },
    Terminal(TerminalCommand),
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt { session_id: SessionId, text: String, content: Option<Vec<ContentBlock>> },
    Cancel { session_id: SessionId },
    SetConfigOption { session_id: SessionId, config_id: String, value: String },
    AuthenticateMcpServer { session_id: SessionId, server_name: String },
    Authenticate { method_id: String },
    ListSessions,
    LoadSession { session_id: SessionId, cwd: PathBuf },
    NewSession { cwd: PathBuf },
    SearchPrompts(PromptSearchParams),
    SessionPreview { session_id: String },
    ListWorkspaces { session_id: String },
    MoveWorkspace { session_id: String, target: WorkspaceMoveTarget },
}

impl AgentCommand {
    pub(crate) fn failure(&self) -> FailedCommand {
        match self {
            Self::Prompt { .. } => FailedCommand::Prompt,
            Self::LoadSession { .. } => FailedCommand::LoadSession,
            Self::ListWorkspaces { .. } => FailedCommand::ListWorkspaces,
            Self::MoveWorkspace { .. } => FailedCommand::MoveWorkspace,
            Self::Cancel { .. } => FailedCommand::Other("cancel"),
            Self::SetConfigOption { .. } => FailedCommand::Other("set config option"),
            Self::AuthenticateMcpServer { .. } => FailedCommand::Other("authenticate MCP server"),
            Self::Authenticate { .. } => FailedCommand::Other("authenticate provider"),
            Self::ListSessions => FailedCommand::Other("list sessions"),
            Self::NewSession { .. } => FailedCommand::Other("create new session"),
            Self::SearchPrompts(_) => FailedCommand::Other("search prompts"),
            Self::SessionPreview { .. } => FailedCommand::Other("preview session"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedCommand {
    Prompt,
    LoadSession,
    ListWorkspaces,
    MoveWorkspace,
    Other(&'static str),
}

impl FailedCommand {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Prompt => "send prompt",
            Self::LoadSession => "load session",
            Self::ListWorkspaces => "list workspaces",
            Self::MoveWorkspace => "move workspace",
            Self::Other(name) => name,
        }
    }
}

#[derive(Debug, Clone)]
pub enum FilesystemCommand {
    IndexFiles { request_id: RequestId, root: PathBuf },
    PrepareSubmission { attachments: Vec<PromptAttachment> },
    ListThemes,
    ApplyTheme { settings: Box<UiSettings>, value: String },
}

#[derive(Debug, Clone)]
pub enum GitCommand {
    Load { request_id: RequestId, working_dir: PathBuf, repo_root: Option<PathBuf>, scope: DiffScope },
    StageFiles { request_id: RequestId, repo_root: PathBuf, paths: Vec<String> },
    UnstageFiles { request_id: RequestId, repo_root: PathBuf, paths: Vec<String> },
    StageAll { request_id: RequestId, repo_root: PathBuf },
    UnstageAll { request_id: RequestId, repo_root: PathBuf },
    Commit { request_id: RequestId, repo_root: PathBuf, message: String },
    DiscardFile { request_id: RequestId, repo_root: PathBuf, path: String, status: FileStatus },
    LoadFullFile { request_id: RequestId, repo_root: PathBuf, path: String },
}

#[derive(Debug, Clone)]
pub enum TerminalCommand {
    RingBell,
}

pub enum CommandResult {
    PromptSearchFailed { query: String, error: String },
    FilesIndexed { request_id: RequestId, files: Vec<FileEntry> },
    GitDiff(GitDiffEvent),
    SubmissionPrepared(AttachmentOutcome),
    ThemesListed(Vec<String>),
    ThemeApplied { settings: Box<UiSettings>, theme: Theme, error: Option<String> },
    WorkspaceResolved { cwd: PathBuf, status: WorkspaceStatus },
    Failed { command: FailedCommand, error: String },
}
