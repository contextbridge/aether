use super::config::build_theme_entries;
use super::{App, ForegroundOperation, Overlay, Route};
use crate::command::{AgentCommand, Command, FilesystemCommand, GitReviewCommand};
use crate::settings::overlay::{SettingsChange, SettingsOverlay};
use crate::surfaces::input::{
    ElicitationOutput, GitReviewOutput, ArtifactReviewOutput, ReviewOutcome, RootOutput, SessionPickerOutput,
    SettingsOutput, WorkspacePickerOutput,
};
use crate::surfaces::picker::CommandEntry;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::v2::SessionId;

pub(super) fn builtin_commands(capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
    let mut commands: Vec<CommandEntry> = [
        ("clear", "Start a new session"),
        ("resume", "Resume a previous session"),
        ("settings", "Configure agent options"),
    ]
    .into_iter()
    .map(builtin)
    .collect();
    if capabilities.workspace_move {
        commands.push(builtin(("move", "Move session to another workspace")));
    }
    commands
}

fn builtin((name, description): (&str, &str)) -> CommandEntry {
    CommandEntry {
        name: name.to_string(),
        description: description.to_string(),
        has_input: false,
        hint: None,
        builtin: true,
    }
}

impl App {
    pub(super) fn dispatch_builtin_command(&mut self, cmd: &CommandEntry) {
        self.composer.clear();
        match cmd.name.as_str() {
            "clear" => {
                if !self.can_start_foreground_operation() {
                    self.notify("Cannot clear while a prompt or session transition is in progress");
                    return;
                }
                self.reset_conversation();
                self.foreground = ForegroundOperation::CreatingSession {
                    previous_selections: self.session.config_options().iter().filter_map(|option| {
                        option.select().map(|select| (option.id.clone(), select.current_value.to_string()))
                    }).collect(),
                };
                self.queue(Command::Agent(AgentCommand::NewSession { cwd: self.session.working_dir().to_path_buf() }));
            }
            "resume" => {
                self.queue(Command::Agent(AgentCommand::ListSessions));
            }
            "settings" => self.open_settings(),
            "move" => self.begin_workspace_move(),
            _ => {}
        }
    }

    fn open_settings(&mut self) {
        let mut overlay = SettingsOverlay::new(
            self.session.config_options(),
            self.session.server_statuses().to_vec(),
            self.session.auth_methods(),
        );
        overlay.upsert_local_entries(build_theme_entries(self.ui.settings.ui(), &[]));
        overlay.add_status_entries();
        self.open_overlay(Overlay::Settings(Box::new(overlay)));
        self.queue(Command::Filesystem(FilesystemCommand::ListThemes));
    }

    fn begin_workspace_move(&mut self) {
        if !self.can_start_foreground_operation() {
            self.notify("Cannot move workspace while a prompt is running or another move is in progress");
            return;
        }
        self.foreground = ForegroundOperation::ListingWorkspaces;
        self.queue(Command::Agent(AgentCommand::ListWorkspaces {
            session_id: self.session.session_id().0.to_string(),
        }));
    }

    pub(super) fn dispatch_outputs(&mut self, outputs: Vec<RootOutput>) {
        for output in outputs {
            self.handle_output(output);
        }
    }

    fn handle_output(&mut self, output: RootOutput) {
        match output {
            RootOutput::Session(output) => match output {
                SessionPickerOutput::Close => self.close_active(),
                SessionPickerOutput::Resume { session_id, cwd } => self.resume_session(&session_id, &cwd),
                SessionPickerOutput::Preview(session_id) => {
                    self.queue(Command::Agent(AgentCommand::SessionPreview { session_id }));
                }
            },
            RootOutput::Workspace(output) => match output {
                WorkspacePickerOutput::Close => self.close_active(),
                WorkspacePickerOutput::Move { target } => {
                    self.close_overlay();
                    self.foreground = ForegroundOperation::MovingWorkspace;
                    self.queue(Command::Agent(AgentCommand::MoveWorkspace {
                        session_id: self.session.session_id().0.to_string(),
                        target,
                    }));
                }
            },
            RootOutput::Settings(output) => match output {
                SettingsOutput::Close => self.close_active(),
                SettingsOutput::SetConfigOption { config_id, value } => {
                    self.set_config_option(&config_id, &value);
                    self.apply_settings_change(&SettingsChange { config_id, new_value: value });
                }
                SettingsOutput::SetTheme(value) => self.apply_theme_change(&value),
                SettingsOutput::AuthenticateServer(name) => {
                    self.queue(Command::Agent(AgentCommand::AuthenticateMcpServer {
                        session_id: self.session.session_id().clone(),
                        server_name: name,
                    }));
                }
                SettingsOutput::AuthenticateProvider(method_id) => {
                    if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                        overlay.on_authenticate_started(&method_id);
                    }
                    self.queue(Command::Agent(AgentCommand::Authenticate { method_id }));
                }
            },
            RootOutput::Elicitation(ElicitationOutput::Close) => self.close_active(),
            RootOutput::ArtifactReview(ArtifactReviewOutput::Approved) => {
                self.notify("Artifact approved");
                self.close_active();
            }
            RootOutput::ArtifactReview(ArtifactReviewOutput::SetTheme(value)) => self.apply_theme_change(&value),
            RootOutput::ArtifactReview(ArtifactReviewOutput::Outcome(outcome)) => {
                if let ReviewOutcome::Submitted(summary) = outcome {
                    self.notify(&summary);
                }
                self.close_active();
            }
            RootOutput::GitReview(output) => match output {
                GitReviewOutput::SetTheme(value) => self.apply_theme_change(&value),
                GitReviewOutput::Outcome(ReviewOutcome::Cancelled) => self.close_active(),
                GitReviewOutput::Outcome(ReviewOutcome::Submitted(prompt)) => self.submit_review(&prompt),
                GitReviewOutput::Command(command) => {
                    self.queue(Command::GitReview(command));
                }
            },
        }
    }

    pub(super) fn open_overlay(&mut self, overlay: Overlay) {
        self.close_overlay();
        self.overlay = Some(overlay);
    }

    pub(super) fn open_route(&mut self, route: Route) {
        if let Route::GitReview(_) = &self.route {
            self.queue(Command::GitReview(GitReviewCommand::Close));
        }
        if matches!(route, Route::GitReview(_) | Route::ArtifactReview(_)) {
            self.queue(Command::Filesystem(FilesystemCommand::ListReviewThemes));
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
        if let Some(overlay) = self.overlay.as_mut() {
            match overlay {
                Overlay::Elicitation(modal) => modal.cancel(),
                Overlay::Settings(overlay) => overlay.cancel_pending_elicitation(),
                Overlay::Sessions(_) | Overlay::Workspaces(_) => {}
            }
        }
        self.overlay = None;
        if matches!(self.foreground, ForegroundOperation::PickingWorkspace) {
            self.foreground = ForegroundOperation::Idle;
        }
    }

    pub(super) fn apply_settings_change(&mut self, change: &SettingsChange) {
        if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.apply_change(change);
        }
        self.session.update_config_option_value(&change.config_id, &change.new_value);
    }

    fn resume_session(&mut self, session_id: &SessionId, cwd: &std::path::Path) {
        if !self.can_start_foreground_operation() {
            self.notify("Cannot resume while a prompt or session transition is in progress");
            return;
        }
        self.reset_conversation();
        self.session.set_session(session_id.clone(), Vec::new());
        self.foreground = ForegroundOperation::ResumingSession {
            session_id: session_id.clone(), cwd: cwd.to_path_buf(),
        };
        self.queue(Command::Agent(AgentCommand::ResumeSession {
            session_id: session_id.clone(),
            cwd: cwd.to_path_buf(),
        }));
        self.return_to_conversation();
    }

    pub(super) fn resolve_workspace(&mut self, cwd: std::path::PathBuf) {
        self.queue(Command::Agent(AgentCommand::FetchWorkspaceStatus {
            session_id: self.session.session_id().0.to_string(),
            cwd,
        }));
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: std::path::PathBuf) {
        self.return_to_conversation();
        self.notify(&format!("Moved to {}", new_cwd.display()));
        let session_id = self.session.session_id().clone();
        self.reset_conversation();
        self.foreground = ForegroundOperation::LoadingWorkspaceSession {
            session_id: session_id.clone(), cwd: new_cwd.clone(),
        };
        self.queue(Command::Agent(AgentCommand::ResumeSession { session_id, cwd: new_cwd.clone() }));
        self.session.set_working_dir(new_cwd.clone());
        self.resolve_workspace(new_cwd);
    }

    /// Sends the review the git-diff screen assembled as a normal prompt.
    pub(super) fn submit_review(&mut self, prompt: &str) {
        if !self.can_start_foreground_operation() {
            return;
        }
        self.conversation.append_notice(format!("[wisp] Submitted review of working tree diff.\n{prompt}"));
        self.start_prompt(prompt.to_string(), None);
        self.close_active();
    }

    /// Re-applies the previous session's selections to a freshly created one, so
    /// a `/clear` keeps the model and mode the user had chosen.
    pub(super) fn restore_config_selections(&mut self, previous: &[(String, String)]) {
        for (config_id, value) in previous {
            let Some(option) = self.session.config_options().iter().find(|option| option.id == *config_id) else {
                continue;
            };
            let Some(select) = option.select() else {
                continue;
            };
            if select.current_value != value {
                self.set_config_option(config_id, value);
            }
        }
    }

    pub(super) fn can_start_foreground_operation(&self) -> bool {
        self.foreground.is_idle() && self.conversation.turn().is_idle()
    }

    /// Adds a semantic notice for information outside the agent's own output.
    pub(super) fn notify(&mut self, message: &str) {
        self.conversation.append_notice(format!("[wisp] {message}"));
    }
}
