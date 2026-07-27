use super::config::{build_theme_entries, update_config_option_value};
use super::{App, Layer, WorkspaceMoveState};
use crate::picker::CommandEntry;
use crate::settings_overlay::{SettingsChange, SettingsOverlay};
use crate::surface::Action;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::{self as acp, SessionId};

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

pub(super) fn merge_builtins(
    agent_commands: Vec<CommandEntry>,
    capabilities: &AetherCapabilities,
) -> Vec<CommandEntry> {
    let mut all = builtin_commands(capabilities);
    all.extend(agent_commands);
    all
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
                self.report_if_err("create new session", self.agent.handle.new_session(&self.agent.working_dir));
            }
            "resume" => {
                self.report_if_err("list sessions", self.agent.handle.list_sessions());
            }
            "settings" => self.open_settings(),
            "move" => self.begin_workspace_move(),
            _ => {}
        }
    }

    fn open_settings(&mut self) {
        let mut overlay =
            SettingsOverlay::new(&self.agent.config_options, self.server_statuses.clone(), &self.agent.auth_methods);
        overlay.add_local_entries(build_theme_entries(&self.ui.settings, &[]));
        overlay.add_status_entries();
        self.open_layer(Layer::Settings(overlay));
        self.spawn(crate::tasks::Task::ListThemes);
    }

    pub(super) fn refresh_settings_themes(&mut self, files: &[String]) {
        let entries = build_theme_entries(&self.ui.settings, files);
        self.with_settings(|overlay| overlay.add_local_entries(entries));
    }

    fn begin_workspace_move(&mut self) {
        if self.turn.prompt_in_flight || !self.workspace_move_state.is_idle() {
            self.notify("Cannot move workspace while a prompt is running or another move is in progress");
            return;
        }
        self.workspace_move_state = WorkspaceMoveState::Listing;
        if self.report_if_err("list workspaces", self.agent.handle.list_workspaces(&self.agent.session_id)).is_none() {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    pub(super) fn dispatch_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            self.handle_action(action);
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Close => self.close_layer(),
            Action::Task(task) => self.spawn(task),
            Action::SubmitReview(prompt) => self.submit_review(&prompt),
            Action::LoadSession { session_id, cwd } => self.load_session(&session_id, &cwd),
            Action::RequestSessionPreview { session_id } => {
                let _ = self.agent.handle.session_preview(&SessionId::new(session_id));
            }
            Action::MoveWorkspace { target } => {
                self.close_layer();
                self.workspace_move_state = WorkspaceMoveState::Moving;
                if self
                    .report_if_err("move workspace", self.agent.handle.move_workspace(&self.agent.session_id, target))
                    .is_none()
                {
                    self.workspace_move_state = WorkspaceMoveState::Idle;
                }
            }
            Action::SetConfigOption { config_id, value } => {
                if self.agent.handle.set_config_option(&self.agent.session_id, &config_id, &value).is_ok() {
                    self.apply_settings_change(&SettingsChange { config_id, new_value: value });
                }
            }
            Action::SetTheme(value) => self.apply_theme_change(&value),
            Action::AuthenticateServer(name) => {
                let _ = self.agent.handle.authenticate_mcp_server(&self.agent.session_id, &name);
            }
            Action::AuthenticateProvider(method_id) => {
                self.with_settings(|overlay| overlay.on_authenticate_started(&method_id));
                let _ = self.agent.handle.authenticate(&method_id);
            }
        }
    }

    /// Opens `layer`, closing whatever was already open.
    pub(super) fn open_layer(&mut self, layer: Layer) {
        self.close_layer();
        self.layer = Some(layer);
    }

    /// Dismisses the open layer. Its surface is cancelled first so it can
    /// release anything it was holding, such as an unanswered elicitation.
    pub(super) fn close_layer(&mut self) {
        if let Some(layer) = self.layer.as_mut() {
            layer.surface().cancel();
        }
        self.layer = None;
        if self.workspace_move_state == WorkspaceMoveState::Picking {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    pub(super) fn apply_settings_change(&mut self, change: &SettingsChange) {
        self.with_settings(|overlay| overlay.apply_change(change));
        update_config_option_value(&mut self.agent.config_options, &change.config_id, &change.new_value);
    }

    fn load_session(&mut self, session_id: &SessionId, cwd: &std::path::Path) {
        self.session_loading_buffer.begin_load(session_id.clone());
        if let Err(error) = self.agent.handle.load_session(session_id, cwd) {
            self.session_loading_buffer.remove(session_id);
            self.notify(&format!("Failed to load session: {error}"));
            return;
        }
        self.close_layer();
        self.reset_conversation();
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: std::path::PathBuf) {
        self.spawn(crate::tasks::Task::ResolveWorkspace { cwd: new_cwd.clone() });
        self.close_layer();
        self.reset_conversation();
        self.notify(&format!("Moved to {}", crate::workspace_status::home_relative_path(&new_cwd)));
        self.workspace_move_state = WorkspaceMoveState::LoadingSession;
        self.session_loading_buffer.begin_load(self.agent.session_id.clone());
        if let Err(error) = self.agent.handle.load_session(&self.agent.session_id, &new_cwd) {
            self.session_loading_buffer.remove(&self.agent.session_id);
            self.notify(&format!("Failed to reload session after move: {error}"));
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
        self.agent.working_dir = new_cwd;
    }

    pub(super) fn finish_workspace_move(&mut self, new_cwd: &std::path::Path, status: WorkspaceStatus) {
        if self.agent.working_dir == new_cwd {
            self.workspace_status = status;
        }
    }

    /// Sends the review the git-diff screen assembled as a normal prompt.
    pub(super) fn submit_review(&mut self, prompt: &str) {
        if self.turn.prompt_in_flight {
            return;
        }
        self.turn.prompt_in_flight = true;
        self.conversation
            .transcript
            .push_user_message(&format!("[wisp-next] Submitted review of working tree diff.\n{prompt}"));
        match self.agent.handle.prompt(&self.agent.session_id, prompt, None) {
            Ok(()) => self.close_layer(),
            Err(error) => {
                tracing::error!("failed to send review prompt: {error}");
                self.turn.prompt_in_flight = false;
                self.notify(&format!("Failed to send review: {error}"));
            }
        }
    }

    /// Re-applies the previous session's selections to a freshly created one, so
    /// a `/clear` keeps the model and mode the user had chosen.
    pub(super) fn restore_config_selections(&mut self, previous: &[(String, String)]) {
        for (config_id, value) in previous {
            let Some(option) = self.agent.config_options.iter().find(|option| option.id.0.as_ref() == config_id) else {
                continue;
            };
            let acp::SessionConfigKind::Select(select) = &option.kind else {
                continue;
            };
            if select.current_value.0.as_ref() != value
                && let Err(error) = self.agent.handle.set_config_option(&self.agent.session_id, config_id, value)
            {
                tracing::warn!("Failed to restore config option {config_id} after new session: {error}");
            }
        }
    }

    /// Writes a `[wisp-next]` line into the transcript, the channel for anything
    /// the UI needs to tell the user outside the agent's own output.
    pub(super) fn notify(&mut self, message: &str) {
        self.conversation.transcript.push_user_message(&format!("[wisp-next] {message}"));
    }

    fn report_if_err<E: std::fmt::Display>(&mut self, action: &str, result: Result<(), E>) -> Option<()> {
        match result {
            Ok(()) => Some(()),
            Err(error) => {
                self.notify(&format!("Failed to {action}: {error}"));
                None
            }
        }
    }
}
