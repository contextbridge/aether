use agent_client_protocol::schema::v2::SessionId;
use std::path::PathBuf;

#[derive(Default)]
pub enum ForegroundOperation {
    #[default]
    Idle,
    PreparingPrompt(String),
    Prompt(PromptPhase),
    CreatingSession { previous_selections: Vec<(String, String)> },
    ResumingSession { session_id: SessionId, cwd: PathBuf },
    ListingWorkspaces,
    PickingWorkspace,
    MovingWorkspace,
    LoadingWorkspaceSession { session_id: SessionId, cwd: PathBuf },
}

pub enum PromptPhase {
    Submitting,
    Running,
    CompletedBeforeAcceptance,
}

impl ForegroundOperation {
    pub(super) fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(super) fn prompt_in_flight(&self) -> bool {
        matches!(self, Self::Prompt(PromptPhase::Submitting | PromptPhase::Running))
    }

    pub(super) fn accept_prompt(&mut self) {
        match self {
            Self::Prompt(PromptPhase::Submitting) => *self = Self::Prompt(PromptPhase::Running),
            Self::Prompt(PromptPhase::CompletedBeforeAcceptance) => *self = Self::Idle,
            _ => {}
        }
    }

    pub(super) fn finish_prompt(&mut self) {
        match self {
            Self::Prompt(PromptPhase::Submitting) => *self = Self::Prompt(PromptPhase::CompletedBeforeAcceptance),
            Self::Prompt(PromptPhase::Running) => *self = Self::Idle,
            _ => {}
        }
    }

    pub(super) fn reject_prompt(&mut self) {
        if matches!(self, Self::PreparingPrompt(_) | Self::Prompt(_)) {
            *self = Self::Idle;
        }
    }

    pub(super) fn clear_conversation(&mut self) {
        if matches!(self, Self::PreparingPrompt(_)) {
            *self = Self::Idle;
        }
        self.finish_prompt();
    }

    pub(super) fn take_prepared_prompt(&mut self) -> Option<String> {
        if !matches!(self, Self::PreparingPrompt(_)) {
            return None;
        }
        let Self::PreparingPrompt(text) = std::mem::take(self) else { unreachable!() };
        Some(text)
    }
}
