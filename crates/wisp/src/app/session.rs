use super::config::build_theme_entries;
use super::{App, Overlay, Route};
use crate::command::{Command, HostCommand};
use crate::settings::overlay::{ProviderLoginStatus, SettingsChange, SettingsOverlay};
use crate::surfaces::input::UiOutput;
use acp_utils::config_option_id::THEME_CONFIG_ID;
use client_core::command_entry::CommandEntry;

impl App {
    pub(super) fn dispatch_builtin_command(&mut self, cmd: &CommandEntry) {
        self.composer.clear();
        let effects = match cmd.name.as_str() {
            "clear" => self.core.new_session(),
            "resume" => self.core.list_sessions(),
            "settings" => {
                self.open_settings();
                return;
            }
            "move" => self.core.begin_workspace_move(),
            _ => return,
        };
        self.apply_effects(effects);
    }

    fn open_settings(&mut self) {
        let session = self.core.session();
        let mut overlay =
            SettingsOverlay::new(session.config_options(), session.server_statuses().to_vec(), session.auth_methods());
        overlay.upsert_local_entries(build_theme_entries(self.ui.settings.ui(), &[]));
        self.open_overlay(Overlay::Settings(Box::new(overlay)));
        self.queue(Command::Host(HostCommand::ListThemes));
    }

    pub(super) fn dispatch_outputs(&mut self, outputs: Vec<UiOutput>) {
        for output in outputs {
            self.handle_output(output);
        }
    }

    fn handle_output(&mut self, output: UiOutput) {
        match output {
            UiOutput::Close => self.close_active(),
            UiOutput::ConfigChange(change) => {
                self.apply_settings_change(&change);
                if change.config_id == THEME_CONFIG_ID {
                    self.apply_theme_change(&change.new_value);
                } else {
                    let effects = self.core.set_config_option(&change.config_id, &change.new_value);
                    self.apply_effects(effects);
                }
            }
            UiOutput::AuthenticateServer(server_name) => {
                let effects = self.core.authenticate_mcp_server(server_name);
                self.apply_effects(effects);
            }
            UiOutput::AuthenticateProvider(method_id) => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.set_provider_status(&method_id, ProviderLoginStatus::Authenticating);
                }
                let effects = self.core.authenticate(method_id);
                self.apply_effects(effects);
            }
            UiOutput::ResumeSession { session_id, cwd } => {
                let effects = self.core.resume_session(&session_id, &cwd);
                self.apply_effects(effects);
            }
            UiOutput::PreviewSession(session_id) => {
                let effects = self.core.preview_session(session_id);
                self.apply_effects(effects);
            }
            UiOutput::MoveWorkspace(target) => {
                self.close_overlay();
                let effects = self.core.move_workspace(target);
                self.apply_effects(effects);
            }
            UiOutput::ArtifactReviewed(summary) => {
                self.notify(&summary);
                self.close_active();
            }
            UiOutput::SubmitReview(prompt) => {
                let effects = self.core.submit_review(&prompt);
                self.apply_effects(effects);
            }
            UiOutput::GitReview(command) => self.queue(Command::GitReview(command)),
        }
    }

    pub(super) fn open_overlay(&mut self, overlay: Overlay) {
        self.close_overlay();
        self.overlay = Some(overlay);
    }

    pub(super) fn open_route(&mut self, route: Route) {
        if let Route::GitReview(_) = &self.route {
            let effects = self.core.close_git_review();
            self.apply_effects(effects);
        }
        if matches!(route, Route::GitReview(_) | Route::ArtifactReview(_)) {
            self.queue(Command::Host(HostCommand::ListReviewThemes));
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
        match self.overlay.take() {
            Some(Overlay::Elicitation(mut modal)) => modal.cancel(),
            Some(Overlay::Settings(mut overlay)) => overlay.cancel_pending_elicitation(),
            Some(Overlay::Workspaces(_)) => self.core.cancel_workspace_pick(),
            Some(Overlay::Sessions(_)) | None => {}
        }
    }

    pub(super) fn return_to_conversation(&mut self) {
        self.open_route(Route::Conversation);
    }

    pub(super) fn apply_settings_change(&mut self, change: &SettingsChange) {
        if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.apply_change(change);
        }
    }

    /// Adds a notice for information outside the agent's own output.
    pub(super) fn notify(&mut self, message: &str) {
        self.core.notify(message);
    }
}
