use super::config::build_theme_entries;
use super::{App, Overlay, Route};
use crate::command::{Command, HostCommand};
use crate::settings::overlay::{ProviderLoginStatus, SettingsChange, SettingsOverlay};
use crate::surfaces::input::UiOutput;
use acp_utils::config_option_id::THEME_CONFIG_ID;
use acp_utils::notifications::AetherCapabilities;
use client_core::command_entry::CommandEntry;
use client_core::session_core::SessionCore;

impl App {
    pub(super) fn dispatch_builtin_command(&mut self, cmd: &CommandEntry) {
        self.composer.clear();
        match cmd.name.as_str() {
            "clear" => self.update_core(SessionCore::new_session),
            "resume" => self.update_core(SessionCore::list_sessions),
            "settings" => self.open_settings(),
            "move" => self.update_core(SessionCore::begin_workspace_move),
            _ => {}
        }
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
                    self.update_core(|core| core.set_config_option(&change.config_id, &change.new_value));
                }
            }
            UiOutput::AuthenticateServer(server_name) => self.update_core(|core| core.authenticate_mcp_server(server_name)),
            UiOutput::AuthenticateProvider(method_id) => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.set_provider_status(&method_id, ProviderLoginStatus::Authenticating);
                }
                self.update_core(|core| core.authenticate(method_id));
            }
            UiOutput::ResumeSession { session_id, cwd } => self.update_core(|core| core.resume_session(&session_id, &cwd)),
            UiOutput::PreviewSession(session_id) => self.update_core(|core| core.preview_session(session_id)),
            UiOutput::MoveWorkspace(target) => {
                self.close_overlay();
                self.update_core(|core| core.move_workspace(target));
            }
            UiOutput::ArtifactReviewed(summary) => {
                self.notify(&summary);
                self.close_active();
            }
            UiOutput::SubmitReview(prompt) => self.update_core(|core| core.submit_review(&prompt)),
            UiOutput::GitReview(command) => self.queue(Command::GitReview(command)),
        }
    }

    pub(super) fn open_overlay(&mut self, overlay: Overlay) {
        self.close_overlay();
        self.overlay = Some(overlay);
    }

    pub(super) fn open_route(&mut self, route: Route) {
        if let Route::GitReview(_) = &self.route {
            self.update_core(SessionCore::close_git_review);
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

/// The commands Wisp runs itself, given what the agent supports.
pub(super) fn builtin_commands(capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
    let mut commands = vec![
        builtin("clear", "Start a new session"),
        builtin("resume", "Resume a previous session"),
        builtin("settings", "Configure agent options"),
    ];
    if capabilities.workspace_move {
        commands.push(builtin("move", "Move session to another workspace"));
    }
    commands
}

fn builtin(name: &str, description: &str) -> CommandEntry {
    CommandEntry { name: name.to_string(), description: description.to_string(), has_input: false, hint: None, builtin: true }
}
