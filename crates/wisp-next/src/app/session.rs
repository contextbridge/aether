use super::config::{build_theme_entries, update_config_option_value};
use super::{
    AetherCapabilities, App, CommandEntry, OverlayLayer, SessionId, SessionPickerMessage, SettingsOverlay,
    SettingsOverlayMessage, WorkspaceMoveState, WorkspacePickerMessage, WorkspaceStatus, acp,
};

pub(super) fn builtin_commands(capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
    let mut commands = vec![
        CommandEntry {
            name: "clear".to_string(),
            description: "Start a new session".to_string(),
            has_input: false,
            hint: None,
            builtin: true,
        },
        CommandEntry {
            name: "resume".to_string(),
            description: "Resume a previous session".to_string(),
            has_input: false,
            hint: None,
            builtin: true,
        },
        CommandEntry {
            name: "settings".to_string(),
            description: "Configure agent options".to_string(),
            has_input: false,
            hint: None,
            builtin: true,
        },
    ];
    if capabilities.workspace_move {
        commands.push(CommandEntry {
            name: "move".to_string(),
            description: "Move session to another workspace".to_string(),
            has_input: false,
            hint: None,
            builtin: true,
        });
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

impl App {
    pub(super) fn dispatch_builtin_command(&mut self, cmd: &CommandEntry) {
        match cmd.name.as_str() {
            "clear" => {
                self.composer.clear();
                if let Err(e) = self.prompt_handle.new_session(&self.working_dir) {
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to create new session: {e}"));
                }
            }
            "resume" => {
                self.composer.clear();
                if let Err(e) = self.prompt_handle.list_sessions() {
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to list sessions: {e}"));
                }
            }
            "settings" => {
                self.composer.clear();
                let mut overlay =
                    SettingsOverlay::new(&self.config_options, self.server_statuses.clone(), self.auth_methods.clone());
                overlay.add_local_entries(build_theme_entries(&self.ui_settings));
                overlay.add_mcp_servers_entry();
                overlay.add_provider_logins_entry();
                self.overlay = OverlayLayer::Settings(overlay);
            }
            "move" => {
                self.composer.clear();
                if self.prompt_in_flight || !self.workspace_move_state.is_idle() {
                    self.transcript.push_user_message(
                        "[wisp-next] Cannot move workspace while a prompt is running or another move is in progress",
                    );
                    return;
                }
                self.workspace_move_state = WorkspaceMoveState::Listing;
                if let Err(e) = self.prompt_handle.list_workspaces(&self.session_id) {
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to list workspaces: {e}"));
                    self.workspace_move_state = WorkspaceMoveState::Idle;
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_session_picker_message(&mut self, msg: SessionPickerMessage) {
        match msg {
            SessionPickerMessage::Close => {
                self.overlay = OverlayLayer::None;
            }
            SessionPickerMessage::LoadSession { session_id, cwd } => {
                self.session_loading_buffer.begin_load(session_id.clone());
                if let Err(e) = self.prompt_handle.load_session(&session_id, &cwd) {
                    self.session_loading_buffer.remove(&session_id);
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to load session: {e}"));
                } else {
                    self.overlay = OverlayLayer::None;
                    self.transcript.clear();
                    self.tool_calls.clear();
                    self.prompt_in_flight = false;
                    self.context_usage = None;
                    self.last_drained_kind = None;
                }
            }
            SessionPickerMessage::RequestPreview { session_id } => {
                let _ = self.prompt_handle.session_preview(&SessionId::new(session_id));
            }
        }
    }

    pub(super) fn handle_workspace_picker_message(&mut self, msg: WorkspacePickerMessage) {
        match msg {
            WorkspacePickerMessage::Close => {
                self.overlay = OverlayLayer::None;
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            WorkspacePickerMessage::Move { target } => {
                self.overlay = OverlayLayer::None;
                self.workspace_move_state = WorkspaceMoveState::Moving;
                if let Err(e) = self.prompt_handle.move_workspace(&self.session_id, target) {
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to move workspace: {e}"));
                    self.workspace_move_state = WorkspaceMoveState::Idle;
                }
            }
        }
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: std::path::PathBuf) {
        self.workspace_status = WorkspaceStatus::resolve(&new_cwd);
        self.screen_router.close();
        self.plan_tracker.clear();
        self.transcript.clear();
        self.tool_calls.clear();
        self.prompt_in_flight = false;
        self.context_usage = None;
        self.last_drained_kind = None;
        self.transcript_generation = self.transcript_generation.wrapping_add(1);
        self.transcript.push_user_message(&format!(
            "[wisp-next] Moved to {}",
            crate::workspace_status::home_relative_path(&new_cwd)
        ));
        self.workspace_move_state = WorkspaceMoveState::LoadingSession;
        self.session_loading_buffer.begin_load(self.session_id.clone());
        if let Err(e) = self.prompt_handle.load_session(&self.session_id, &new_cwd) {
            self.session_loading_buffer.remove(&self.session_id);
            self.transcript.push_user_message(&format!("[wisp-next] Failed to reload session after move: {e}"));
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
        self.working_dir = new_cwd;
    }

    pub(super) fn handle_settings_message(&mut self, msg: SettingsOverlayMessage) {
        match msg {
            SettingsOverlayMessage::Close => {
                if let OverlayLayer::Settings(mut overlay) = std::mem::take(&mut self.overlay) {
                    overlay.cancel_pending_elicitation();
                }
            }
            SettingsOverlayMessage::SetConfigOption { config_id, value } => {
                if self.prompt_handle.set_config_option(&self.session_id, &config_id, &value).is_ok() {
                    if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                        overlay.apply_change(&crate::settings_overlay::SettingsChange {
                            config_id: config_id.clone(),
                            new_value: value.clone(),
                        });
                    }
                    update_config_option_value(&mut self.config_options, &config_id, &value);
                }
            }
            SettingsOverlayMessage::SetTheme(value) => {
                self.apply_theme_change(&value);
            }
            SettingsOverlayMessage::AuthenticateServer(name) => {
                let _ = self.prompt_handle.authenticate_mcp_server(&self.session_id, &name);
            }
            SettingsOverlayMessage::AuthenticateProvider(method_id) => {
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.on_authenticate_started(&method_id);
                }
                let _ = self.prompt_handle.authenticate(&method_id);
            }
        }
    }

    pub(super) fn restore_config_selections(&mut self, previous: &[(String, String)]) {
        for (config_id, value) in previous {
            if let Some(option) = self.config_options.iter().find(|o| o.id.0.as_ref() == config_id) {
                let current_value = match &option.kind {
                    acp::SessionConfigKind::Select(s) => s.current_value.0.to_string(),
                    _ => continue,
                };
                if current_value != *value
                    && let Err(e) = self.prompt_handle.set_config_option(&self.session_id, config_id, value)
                {
                    tracing::warn!("Failed to restore config option {config_id} after new session: {e}");
                }
            }
        }
    }
}
