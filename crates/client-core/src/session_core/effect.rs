use crate::command::{Command, Reply};
use crate::composer::submission::Submission;
use crate::session_model::SessionSetting;
use acp_utils::notifications::WorkspaceEntry;

#[derive(Debug)]
pub enum Effect {
    Dispatch(Command),
    ElicitationRequested,
    ElicitationDismissed,
    TurnFinished,
    RestoreSubmission(Submission),
    SessionChanged,
    SessionSettingsChanged(SessionSetting),
    ReviewSubmitted,
    GitReviewChanged,
    WorkspacesListed(Vec<WorkspaceEntry>),
    PromptSearchUpdated,
    ConnectionClosed,
    Reply(Reply),
}
