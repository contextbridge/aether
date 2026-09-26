use agent_client_protocol::schema::v2::SessionId;
use std::path::PathBuf;

/// The operation the UI itself is in the middle of. A prompt's own lifecycle
/// lives in the conversation's turn.
#[derive(Default)]
pub enum ForegroundOperation {
    #[default]
    Idle,
    PreparingPrompt(String),
    CreatingSession { previous_selections: Vec<(String, String)> },
    ResumingSession { session_id: SessionId, cwd: PathBuf },
    ListingWorkspaces,
    PickingWorkspace,
    MovingWorkspace,
    LoadingWorkspaceSession { session_id: SessionId, cwd: PathBuf },
}

impl ForegroundOperation {
    pub(super) fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Abandons a prompt still being prepared, whose conversation or submission is gone.
    pub(super) fn drop_prepared_prompt(&mut self) {
        if matches!(self, Self::PreparingPrompt(_)) {
            *self = Self::Idle;
        }
    }

    pub(super) fn take_prepared_prompt(&mut self) -> Option<String> {
        if !matches!(self, Self::PreparingPrompt(_)) {
            return None;
        }
        let Self::PreparingPrompt(text) = std::mem::take(self) else { unreachable!() };
        Some(text)
    }
}
