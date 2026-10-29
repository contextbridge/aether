use crate::composer::submission::Submission;
use agent_client_protocol::schema::v2::SessionId;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub enum ForegroundOperation {
    #[default]
    Idle,
    PreparingPrompt(Submission),
    CreatingSession {
        previous_selections: Vec<(String, String)>,
    },
    ResumingSession {
        session_id: SessionId,
        cwd: PathBuf,
        moved: bool,
    },
    ListingWorkspaces,
    PickingWorkspace,
    MovingWorkspace,
}

impl ForegroundOperation {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(crate) fn take_prepared_prompt(&mut self) -> Option<Submission> {
        match std::mem::take(self) {
            Self::PreparingPrompt(submission) => Some(submission),
            other => {
                *self = other;
                None
            }
        }
    }
}
