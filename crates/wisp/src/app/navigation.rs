use super::App;
use super::config::build_theme_entries;
use crate::command::HostCommand;
use crate::renderer::DrawContext;
use crate::screens::artifact_review::ArtifactReviewScreen;
use crate::screens::git_diff::GitDiffScreen;
use crate::settings::overlay::{SettingsChange, SettingsOverlay};
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::session_picker::SessionPicker;
use crate::surfaces::workspace_picker::WorkspacePicker;
use client_core::elicitation::{ElicitationAnswer, ElicitationId};
use client_core::session_core::SessionCore;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

/// The full-screen destinations in the application.
pub enum Route {
    Conversation,
    GitReview(Box<GitDiffScreen>),
    ArtifactReview(Box<ArtifactReviewScreen>),
}

/// The transient destination above the active route. Only one overlay receives
/// normal input at a time.
pub enum Overlay {
    Settings(Box<SettingsOverlay>),
    Sessions(SessionPicker),
    Workspaces(WorkspacePicker),
    Elicitation(ElicitationModal),
}

impl Route {
    pub fn is_fullscreen(&self) -> bool {
        !matches!(self, Self::Conversation)
    }
}

impl App {
    pub fn render_route(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match &mut self.route {
            Route::Conversation => None,
            Route::GitReview(screen) => screen.render(area, buf, cx),
            Route::ArtifactReview(screen) => screen.render(area, buf, cx),
        }
    }

    pub fn render_overlay(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match self.overlay.as_mut() {
            Some(Overlay::Settings(overlay)) => overlay.render(area, buf, cx, self.core.session()),
            Some(Overlay::Sessions(picker)) => picker.render(area, buf, cx),
            Some(Overlay::Workspaces(picker)) => picker.render(area, buf, cx),
            Some(Overlay::Elicitation(modal)) => modal.render(area, buf, cx),
            None => None,
        }
    }

    pub fn needs_mouse_capture(&self) -> bool {
        match self.overlay.as_ref() {
            Some(Overlay::Elicitation(modal)) => modal.needs_mouse_capture(),
            Some(Overlay::Settings(overlay)) => overlay.needs_mouse_capture(),
            Some(_) => true,
            None => match self.route {
                Route::Conversation => self.composer.has_open_overlay(),
                Route::GitReview(_) | Route::ArtifactReview(_) => true,
            },
        }
    }

    pub(super) fn open_overlay(&mut self, overlay: Overlay) {
        self.close_overlay();
        self.overlay = Some(overlay);
    }

    pub(super) fn open_route(&mut self, route: Route) {
        match &self.route {
            Route::GitReview(_) => self.update_core(SessionCore::close_git_review),
            Route::ArtifactReview(screen) => self.cancel_elicitation(screen.id()),
            Route::Conversation => {}
        }
        if route.is_fullscreen() {
            self.queue(HostCommand::ListReviewThemes);
        }
        self.close_overlay();
        self.route = route;
    }

    pub(super) fn close_active(&mut self) {
        if self.overlay.is_some() {
            self.close_overlay();
        } else {
            self.return_to_conversation();
        }
    }

    pub(super) fn close_overlay(&mut self) {
        self.cancel_overlay_elicitation();
        if let Some(Overlay::Workspaces(_)) = self.overlay.take() {
            self.update_core(SessionCore::cancel_workspace_pick);
        }
    }

    pub(super) fn cancel_overlay_elicitation(&mut self) {
        let id = match self.overlay.as_mut() {
            Some(Overlay::Elicitation(modal)) => Some(modal.id()),
            Some(Overlay::Settings(overlay)) => overlay.take_pending_elicitation(),
            Some(Overlay::Sessions(_) | Overlay::Workspaces(_)) | None => None,
        };
        if let Some(id) = id {
            self.cancel_elicitation(id);
        }
    }

    pub(super) fn cancel_elicitation(&mut self, id: ElicitationId) {
        self.update_core(|core| core.answer_elicitation(id, ElicitationAnswer::Cancel));
    }

    /// Answers `id`, closing the modal that was asking it.
    pub(super) fn answer_elicitation(&mut self, id: ElicitationId, answer: ElicitationAnswer) {
        match self.overlay.as_mut() {
            Some(Overlay::Elicitation(modal)) if modal.id() == id => self.overlay = None,
            Some(Overlay::Settings(overlay)) => overlay.dismiss_elicitation(id),
            _ => {}
        }
        self.update_core(|core| core.answer_elicitation(id, answer));
    }

    /// The modal still asking `id`, wherever it is drawn.
    pub(super) fn elicitation_modal_mut(&mut self, id: ElicitationId) -> Option<&mut ElicitationModal> {
        match self.overlay.as_mut()? {
            Overlay::Elicitation(modal) => Some(modal),
            Overlay::Settings(overlay) => overlay.pending_elicitation_mut(),
            Overlay::Sessions(_) | Overlay::Workspaces(_) => None,
        }
        .filter(|modal| modal.id() == id)
    }

    pub(super) fn return_to_conversation(&mut self) {
        self.open_route(Route::Conversation);
    }

    pub(super) fn apply_settings_change(&mut self, change: &SettingsChange) {
        if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.apply_change(change);
        }
    }

    pub(super) fn open_settings(&mut self) {
        let mut overlay = SettingsOverlay::new(self.core.session());
        overlay.upsert_local_entries(build_theme_entries(self.ui.settings.ui(), &[]));
        self.open_overlay(Overlay::Settings(Box::new(overlay)));
        self.queue(HostCommand::ListThemes);
    }
}
