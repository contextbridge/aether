use super::config::{build_theme_entries, update_config_option_value};
use super::{
    AetherCapabilities, App, CommandEntry, SessionId, SettingsOverlay, Surface, SurfaceMessage, WorkspaceMoveState,
    WorkspaceStatus, acp,
};
use crate::settings_overlay::SettingsChange;

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
                self.report_if_err("create new session", self.prompt_handle.new_session(&self.working_dir));
            }
            "resume" => {
                self.report_if_err("list sessions", self.prompt_handle.list_sessions());
            }
            "settings" => self.open_settings(),
            "move" => self.begin_workspace_move(),
            _ => {}
        }
    }

    fn open_settings(&mut self) {
        let mut overlay = SettingsOverlay::new(&self.config_options, self.server_statuses.clone(), &self.auth_methods);
        overlay.add_local_entries(build_theme_entries(&self.ui_settings));
        overlay.add_status_entries();
        self.open_layer(Box::new(overlay));
    }

    fn begin_workspace_move(&mut self) {
        if self.turn.prompt_in_flight || !self.workspace_move_state.is_idle() {
            self.notify("Cannot move workspace while a prompt is running or another move is in progress");
            return;
        }
        self.workspace_move_state = WorkspaceMoveState::Listing;
        if self.report_if_err("list workspaces", self.prompt_handle.list_workspaces(&self.session_id)).is_none() {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    pub(super) fn handle_surface_messages(&mut self, messages: Vec<SurfaceMessage>) {
        for message in messages {
            self.handle_surface_message(message);
        }
    }

    fn handle_surface_message(&mut self, message: SurfaceMessage) {
        match message {
            // `Back` is handled by whichever surface opened the one that sent
            // it, so it never reaches here.
            SurfaceMessage::Back | SurfaceMessage::Close => self.close_layer(),
            SurfaceMessage::Effect(effect) => self.pending_effects.push_back(effect),
            SurfaceMessage::SubmitReview(prompt) => self.submit_review(&prompt),
            SurfaceMessage::LoadSession { session_id, cwd } => self.load_session(&session_id, &cwd),
            SurfaceMessage::RequestSessionPreview { session_id } => {
                let _ = self.prompt_handle.session_preview(&SessionId::new(session_id));
            }
            SurfaceMessage::MoveWorkspace { target } => {
                self.close_layer();
                self.workspace_move_state = WorkspaceMoveState::Moving;
                if self
                    .report_if_err("move workspace", self.prompt_handle.move_workspace(&self.session_id, target))
                    .is_none()
                {
                    self.workspace_move_state = WorkspaceMoveState::Idle;
                }
            }
            SurfaceMessage::SetConfigOption { config_id, value } => {
                if self.prompt_handle.set_config_option(&self.session_id, &config_id, &value).is_ok() {
                    self.apply_settings_change(&SettingsChange { config_id, new_value: value });
                }
            }
            SurfaceMessage::SetTheme(value) => self.apply_theme_change(&value),
            SurfaceMessage::AuthenticateServer(name) => {
                let _ = self.prompt_handle.authenticate_mcp_server(&self.session_id, &name);
            }
            SurfaceMessage::AuthenticateProvider(method_id) => {
                if let Some(overlay) = self.layer_as::<SettingsOverlay>() {
                    overlay.on_authenticate_started(&method_id);
                }
                let _ = self.prompt_handle.authenticate(&method_id);
            }
        }
    }

    /// Opens `surface`, closing whatever was already open.
    pub(super) fn open_layer(&mut self, surface: Box<dyn Surface>) {
        self.close_layer();
        self.layer = Some(surface);
    }

    /// Dismisses the open surface. It is cancelled first so it can release
    /// anything it was holding, such as an unanswered elicitation.
    pub(super) fn close_layer(&mut self) {
        if let Some(surface) = self.layer.as_deref_mut() {
            surface.cancel();
        }
        self.layer = None;
        if self.workspace_move_state == WorkspaceMoveState::Picking {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    pub(super) fn apply_settings_change(&mut self, change: &SettingsChange) {
        if let Some(overlay) = self.layer_as::<SettingsOverlay>() {
            overlay.apply_change(change);
        }
        update_config_option_value(&mut self.config_options, &change.config_id, &change.new_value);
    }

    fn load_session(&mut self, session_id: &SessionId, cwd: &std::path::Path) {
        self.session_loading_buffer.begin_load(session_id.clone());
        if let Err(error) = self.prompt_handle.load_session(session_id, cwd) {
            self.session_loading_buffer.remove(session_id);
            self.notify(&format!("Failed to load session: {error}"));
            return;
        }
        self.close_layer();
        self.reset_conversation();
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: std::path::PathBuf) {
        self.workspace_status = WorkspaceStatus::resolve(&new_cwd);
        self.close_layer();
        self.reset_conversation();
        self.notify(&format!("Moved to {}", crate::workspace_status::home_relative_path(&new_cwd)));
        self.workspace_move_state = WorkspaceMoveState::LoadingSession;
        self.session_loading_buffer.begin_load(self.session_id.clone());
        if let Err(error) = self.prompt_handle.load_session(&self.session_id, &new_cwd) {
            self.session_loading_buffer.remove(&self.session_id);
            self.notify(&format!("Failed to reload session after move: {error}"));
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
        self.working_dir = new_cwd;
    }

    /// Sends the review the git-diff screen assembled as a normal prompt.
    pub(super) fn submit_review(&mut self, prompt: &str) {
        if self.turn.prompt_in_flight {
            return;
        }
        self.turn.prompt_in_flight = true;
        self.transcript.push_user_message(&format!("[wisp-next] Submitted review of working tree diff.\n{prompt}"));
        match self.prompt_handle.prompt(&self.session_id, prompt, None) {
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
            let Some(option) = self.config_options.iter().find(|option| option.id.0.as_ref() == config_id) else {
                continue;
            };
            let acp::SessionConfigKind::Select(select) = &option.kind else {
                continue;
            };
            if select.current_value.0.as_ref() != value
                && let Err(error) = self.prompt_handle.set_config_option(&self.session_id, config_id, value)
            {
                tracing::warn!("Failed to restore config option {config_id} after new session: {error}");
            }
        }
    }

    /// Writes a `[wisp-next]` line into the transcript, the channel for anything
    /// the UI needs to tell the user outside the agent's own output.
    pub(super) fn notify(&mut self, message: &str) {
        self.transcript.push_user_message(&format!("[wisp-next] {message}"));
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
