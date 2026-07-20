use crate::screens::git_diff::GitDiffScreen;
use crate::screens::plan_review::PlanReviewScreen;
use crate::settings::overlay::SettingsOverlay;
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::session_picker::SessionPicker;
use crate::surfaces::workspace_picker::WorkspacePicker;

/// The full-screen destinations in the application.
pub enum Route {
    Conversation,
    GitReview(Box<GitDiffScreen>),
    PlanReview(Box<PlanReviewScreen>),
}

impl Route {
    pub fn is_fullscreen(&self) -> bool {
        !matches!(self, Self::Conversation)
    }
}

/// The transient destination above the active route. Only one overlay receives
/// normal input at a time.
pub enum Overlay {
    Settings(Box<SettingsOverlay>),
    Sessions(SessionPicker),
    Workspaces(WorkspacePicker),
    Elicitation(ElicitationModal),
}
