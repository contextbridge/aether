use crate::command::HostCommand;
use crate::settings::overlay::SettingsChange;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_client::DiffReviewEvent;
use client_core::elicitation::{ElicitationAnswer, ElicitationId};
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
    PreviewSession(SessionId),
    MoveWorkspace(WorkspaceMoveTarget),
    AnswerElicitation { id: ElicitationId, answer: ElicitationAnswer },
    ArtifactReviewed(String),
    SubmitReview(String),
    ReviewEvent(DiffReviewEvent),
    Host(HostCommand),
}

pub(crate) fn one<T>(action: Option<T>) -> Vec<T> {
    action.into_iter().collect()
}
