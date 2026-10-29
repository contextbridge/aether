use super::{App, ExitState, Overlay, Route};
use crate::command::Command;
use crate::screens::artifact_review::ArtifactReviewScreen;
use crate::settings::overlay::ProviderLoginStatus;
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::session_picker::SessionPicker;
use crate::surfaces::workspace_picker::WorkspacePicker;
use client_core::command::Reply;
use client_core::elicitation::Elicitation;
use client_core::session_core::{Effect, SessionCore};

impl App {
    pub(super) fn update_core<T>(&mut self, change: impl FnOnce(&mut SessionCore) -> T) -> T {
        let value = change(&mut self.core);
        for effect in self.core.take_effects() {
            self.apply_effect(effect);
        }
        value
    }

    pub(super) fn queue(&mut self, command: impl Into<Command>) {
        self.commands.push_back(command.into());
    }

    fn apply_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Dispatch(command) => self.queue(command),
            Effect::ElicitationRequested => self.show_elicitation(),
            Effect::ElicitationDismissed => self.close_elicitation_owner(),
            Effect::TurnFinished => {
                if let Err(error) = (self.platform.bell)() {
                    self.update_core(|core| core.report_error(&error));
                }
            }
            Effect::RestoreSubmission(submission) => self.composer.restore_submission(submission),
            Effect::SessionChanged => self.return_to_conversation(),
            Effect::SessionSettingsChanged(setting) => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_setting_changed(setting, self.core.session());
                }
            }
            Effect::ReviewSubmitted => self.close_active(),
            Effect::GitReviewChanged => {
                if let (Route::GitReview(screen), Some(state)) = (&mut self.route, self.core.git_review()) {
                    screen.install(state);
                }
            }
            Effect::WorkspacesListed(workspaces) => {
                self.open_overlay(Overlay::Workspaces(WorkspacePicker::new(workspaces)));
            }
            Effect::PromptSearchUpdated => {
                self.composer.show_prompt_search_results(self.core.prompt_search().results());
            }
            Effect::ConnectionClosed => self.on_connection_closed(),
            Effect::Reply(reply) => self.apply_reply(reply),
        }
    }

    fn apply_reply(&mut self, reply: Reply) {
        match reply {
            Reply::GitReviewAction(result) => {
                if let Route::GitReview(screen) = &mut self.route {
                    screen.on_action_result(result);
                }
            }
            Reply::GitReviewLookup(reply) => {
                if let Route::GitReview(screen) = &mut self.route {
                    let outputs = screen.on_lookup_result(reply);
                    self.dispatch_outputs(outputs);
                }
            }
            Reply::SessionsListed(sessions) => {
                let picker = SessionPicker::new(sessions, self.core.session().capabilities().session_preview);
                if let Some(id) = picker.initial_preview_request() {
                    self.update_core(|core| core.preview_session(id));
                }
                self.open_overlay(Overlay::Sessions(picker));
            }
            Reply::SessionPreviewLoaded { session_id, result } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    match result {
                        Ok(preview) => picker.on_preview_loaded(preview),
                        Err(error) => picker.on_preview_failed(&session_id, error),
                    }
                }
            }
            Reply::AuthenticationCompleted { method_id, result } => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    let status =
                        if result.is_ok() { ProviderLoginStatus::LoggedIn } else { ProviderLoginStatus::NeedsLogin };
                    overlay.set_provider_status(&method_id, status);
                }
            }
        }
    }

    fn show_elicitation(&mut self) {
        self.close_elicitation_owner();
        let Some((id, elicitation)) = self.core.elicitation() else {
            return;
        };
        let modal = match elicitation.clone() {
            Elicitation::ArtifactReview(meta) => {
                self.open_route(Route::ArtifactReview(Box::new(ArtifactReviewScreen::new(id, meta))));
                return;
            }
            Elicitation::Form { source, message, fields } => ElicitationModal::form(id, source, message, fields),
            Elicitation::Url { source, message, url } => {
                ElicitationModal::url(id, source, message, url, &self.platform)
            }
        };
        match self.overlay.as_mut() {
            Some(Overlay::Settings(overlay)) => overlay.show_elicitation(modal),
            _ => self.open_overlay(Overlay::Elicitation(modal)),
        }
    }

    fn on_connection_closed(&mut self) {
        self.return_to_conversation();
        self.exit_state = ExitState::ConnectionLost;
    }

    fn close_elicitation_owner(&mut self) {
        if matches!(self.overlay, Some(Overlay::Elicitation(_))) {
            self.close_overlay();
        } else {
            self.cancel_overlay_elicitation();
        }
        if matches!(self.route, Route::ArtifactReview(_)) {
            self.return_to_conversation();
        }
    }
}
