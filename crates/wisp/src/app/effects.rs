use super::{App, ExitState, Overlay, Route};
use crate::command::{Command, CommandResult, HostCommand};
use crate::screens::artifact_review::ArtifactReviewScreen;
use crate::settings::overlay::ProviderLoginStatus;
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::session_picker::SessionPicker;
use crate::surfaces::workspace_picker::WorkspacePicker;
use agent_client_protocol::schema::v2 as acp;
use client_core::elicitation::ElicitationResponder;
use client_core::session_core::{Effect, Elicitation, SessionCore};

impl App {
    /// Runs `change` on the session core, then turns the effects it produced into commands,
    /// routes, and overlays, in the order the core produced them.
    pub(super) fn update_core<T>(&mut self, change: impl FnOnce(&mut SessionCore) -> T) -> T {
        let value = change(&mut self.core);
        for effect in self.core.take_effects() {
            self.apply_effect(effect);
        }
        value
    }

    /// Reduces the command results the core hands back for the UI to show.
    pub(super) fn on_unhandled_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::Host(result) => self.on_host_result(result),
            CommandResult::GitReview(state) => {
                if let Route::GitReview(screen) = &mut self.route {
                    screen.install(&state);
                }
            }
            CommandResult::GitReviewAction(result) => {
                if let Route::GitReview(screen) = &mut self.route {
                    screen.on_action_result(result);
                }
            }
            CommandResult::SessionsListed(Ok(response)) => self.open_session_picker(response.sessions),
            CommandResult::SessionPreviewLoaded { session_id, result } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    match result {
                        Ok(preview) => picker.on_preview_loaded(preview),
                        Err(error) => picker.on_preview_failed(&session_id, error),
                    }
                }
            }
            CommandResult::WorkspacesListed(Ok(response)) => {
                self.open_overlay(Overlay::Workspaces(WorkspacePicker::new(response.workspaces)));
            }
            CommandResult::PromptSearchResults { query, result } => match result {
                Ok(response) => self.composer.prompt_search_on_results(response),
                Err(error) => {
                    if let Some(picker) = self.composer.prompt_search_mut() {
                        picker.on_failed(&query, error);
                    }
                }
            },
            CommandResult::FilesIndexed { request_id, files } => self.composer.on_files_indexed(request_id, files),
            CommandResult::AuthenticationCompleted { method_id, result } => {
                if let Err(error) = &result {
                    tracing::warn!("Provider authentication failed for {method_id}: {error}");
                }
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    let status =
                        if result.is_ok() { ProviderLoginStatus::LoggedIn } else { ProviderLoginStatus::NeedsLogin };
                    overlay.set_provider_status(&method_id, status);
                }
            }
            _ => {}
        }
    }

    fn apply_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Dispatch(command) => self.queue(command.widen()),
            Effect::ShowElicitation { elicitation, responder } => self.show_elicitation(elicitation, responder),
            Effect::TurnFinished => self.queue(Command::Host(HostCommand::RingBell)),
            Effect::ConfigOptionsChanged => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_config_options(self.core.session().config_options());
                }
            }
            Effect::AuthMethodsChanged => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_auth_methods(self.core.session().auth_methods());
                }
            }
            Effect::ServerStatusesChanged => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_server_statuses(self.core.session().server_statuses().to_vec());
                }
            }
            Effect::RestoreSubmission(submission) => self.composer.restore_submission(submission),
            Effect::SessionChanged => self.return_to_conversation(),
            Effect::ReviewSubmitted => self.close_active(),
            Effect::ConnectionClosed => self.on_connection_closed(),
        }
    }

    fn show_elicitation(&mut self, elicitation: Elicitation, responder: ElicitationResponder) {
        self.close_elicitation_owner();
        let params = match elicitation {
            Elicitation::ArtifactReview(meta) => {
                self.open_route(Route::ArtifactReview(Box::new(ArtifactReviewScreen::new(meta, responder))));
                return;
            }
            Elicitation::Request(params) => *params,
        };
        // The settings overlay answers its own elicitations in place so
        // an OAuth prompt does not tear down the pane that started it.
        if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.on_elicitation_request(params, responder, self.browser_opener.clone(), self.clipboard_writer.clone());
            return;
        }
        if let Some(modal) =
            ElicitationModal::with_url_handlers(params, responder, self.browser_opener.clone(), self.clipboard_writer.clone())
        {
            self.open_overlay(Overlay::Elicitation(modal));
        }
    }

    /// Offers the agent's sessions other than the current one.
    fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        let current = self.core.session().session_id().clone();
        let sessions = sessions.into_iter().filter(|session| session.session_id != current).collect();
        let picker = SessionPicker::new(sessions, self.core.session().capabilities().session_preview);
        if let Some(id) = picker.initial_preview_request() {
            self.update_core(|core| core.preview_session(id));
        }
        self.open_overlay(Overlay::Sessions(picker));
    }

    /// The connection is gone, but a remote agent may still be running.
    /// Release local interactions and overlays, then ask the event loop to exit.
    fn on_connection_closed(&mut self) {
        self.return_to_conversation();
        self.commands.retain(|command| !matches!(command, Command::Host(HostCommand::RingBell)));
        self.exit_state = ExitState::ConnectionLost;
    }

    /// Answers any elicitation the current route or overlay is holding, leaving the
    /// settings overlay itself open so its pane survives.
    fn close_elicitation_owner(&mut self) {
        match self.overlay.as_mut() {
            Some(Overlay::Settings(overlay)) => overlay.cancel_pending_elicitation(),
            Some(Overlay::Elicitation(_)) => self.close_overlay(),
            _ => {}
        }
    }
}
