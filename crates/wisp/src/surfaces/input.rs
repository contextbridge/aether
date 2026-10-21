use client_core::command::GitReviewCommand;
use crate::settings::overlay::SettingsChange;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v2::SessionId;
use std::path::PathBuf;

/// What a surface asks of the app. Every overlay, route and pane reports
/// through this one enum, so the app acts on each request in one place.
#[derive(Debug)]
pub enum UiOutput {
    Close,
    ConfigChange(SettingsChange),
    AuthenticateServer(String),
    AuthenticateProvider(String),
    ResumeSession { session_id: SessionId, cwd: PathBuf },
    PreviewSession(String),
    MoveWorkspace(WorkspaceMoveTarget),
    ArtifactReviewed(String),
    SubmitReview(String),
    GitReview(GitReviewCommand),
}

pub(crate) fn one<T>(action: Option<T>) -> Vec<T> {
    action.into_iter().collect()
}
