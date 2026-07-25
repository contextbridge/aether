use super::config::extract_config_selections;
use super::session::merge_builtins;
use super::{
    AcpEvent, App, CommandEntry, ContextUsageDisplay, ElicitationModal, ExitState, Instant, Layer, McpNotification,
    SessionId, SessionPicker, SettingsOverlay, ToolStatus, WorkspaceMoveState, WorkspacePicker, acp,
};
use crate::screens::plan_review::PlanReviewScreen;

impl App {
    pub fn on_acp_event(&mut self, event: AcpEvent) {
        self.on_acp_event_inner(event);
        self.refresh_progress();
    }

    fn on_acp_event_inner(&mut self, event: AcpEvent) {
        match event {
            AcpEvent::SessionUpdate { session_id, update } => {
                if let Some(passthrough) = self.session_loading_buffer.push(&session_id, *update) {
                    self.on_session_update(&passthrough);
                }
            }
            AcpEvent::PromptDone(stop_reason) => {
                let status = match stop_reason {
                    acp::StopReason::Cancelled => ToolStatus::Error("cancelled".to_string()),
                    _ => ToolStatus::Success,
                };
                self.finish_prompt(&status);
            }
            AcpEvent::PromptError(error) => {
                tracing::error!("Prompt error: {error}");
                self.finish_prompt(&ToolStatus::Error(format!("failed: {error}")));
                self.notify(&format!("Prompt failed: {error}"));
            }
            AcpEvent::ContextUsage(params) => {
                self.turn.context_usage =
                    params.usage.context_limit.map(|limit| ContextUsageDisplay::new(params.usage.input_tokens, limit));
            }
            AcpEvent::ContextCompaction(params) => {
                self.turn.compaction_active = params.active;
            }
            AcpEvent::ContextCleared(_) => {
                self.reset_conversation();
                self.notify("Context cleared");
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                self.close_elicitation_owner();
                if let Some(meta) = plan_review_meta(&params) {
                    self.open_screen(Box::new(PlanReviewScreen::new(meta, responder)));
                    return;
                }
                // The settings overlay answers its own elicitations in place so
                // an OAuth prompt does not tear down the pane that started it.
                if let Layer::Settings(overlay) = &mut self.layer {
                    overlay.on_elicitation_request(params, responder);
                    return;
                }
                self.open_layer(Layer::Elicitation(ElicitationModal::new(params, responder)));
            }
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(&notification),
            AcpEvent::AuthMethodsUpdated(params) => {
                self.auth_methods.clone_from(&params.auth_methods);
                self.with_settings(|overlay| overlay.update_auth_methods(&params.auth_methods));
            }
            AcpEvent::AuthenticateComplete { method_id } => {
                self.with_settings(|overlay| overlay.on_authenticate_complete(&method_id));
            }
            AcpEvent::AuthenticateFailed { method_id, error } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
                self.with_settings(|overlay| overlay.on_authenticate_failed(&method_id));
            }
            AcpEvent::ConnectionClosed => self.on_connection_closed(),
            AcpEvent::ConfigOptionUpdateFailed { error } => {
                tracing::warn!("set_session_config_option failed: {error}");
                self.notify(&format!("Failed to update setting: {error}"));
            }
            AcpEvent::SessionsListed { sessions } => self.open_session_picker(sessions),
            AcpEvent::SessionLoaded { session_id, config_options } => {
                self.on_session_loaded(session_id, config_options);
            }
            AcpEvent::NewSessionCreated { session_id, config_options } => {
                self.on_new_session(session_id, config_options);
            }
            AcpEvent::SessionPreviewLoaded(preview) => {
                self.with_session_picker(|picker| picker.on_preview_loaded(preview));
            }
            AcpEvent::SessionPreviewFailed { session_id, error } => {
                self.with_session_picker(|picker| picker.on_preview_failed(&session_id, error));
            }
            AcpEvent::PromptSearchResults(response) => {
                self.composer.prompt_search_on_results(response);
            }
            AcpEvent::PromptSearchFailed { query: _, search_generation, error } => {
                if let Some(picker) = self.composer.prompt_search() {
                    picker.on_failed(search_generation, error);
                }
            }
            AcpEvent::WorkspacesListed(response) => {
                self.open_layer(Layer::WorkspacePicker(WorkspacePicker::new(response.workspaces)));
                self.workspace_move_state = WorkspaceMoveState::Picking;
            }
            AcpEvent::WorkspaceMoved(response) => self.on_workspace_moved(response.new_cwd),
            AcpEvent::WorkspaceListFailed { error } => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            AcpEvent::WorkspaceMoveFailed { error } => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            AcpEvent::SubAgentProgress(progress) => {
                self.tool_calls.on_sub_agent_progress(&progress);
                if self.tool_calls.has_tool(&progress.parent_tool_id) {
                    self.transcript.ensure_tool_segment(&progress.parent_tool_id);
                }
            }
        }
    }

    /// Reports why a workspace move could not proceed and leaves move mode.
    fn abandon_workspace_move(&mut self, message: &str) {
        self.notify(message);
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    /// Runs `apply` when the settings overlay is open. Agent pushes that only
    /// affect what it displays are dropped when it is not.
    fn with_settings(&mut self, apply: impl FnOnce(&mut SettingsOverlay)) {
        if let Layer::Settings(overlay) = &mut self.layer {
            apply(overlay);
        }
    }

    fn with_session_picker(&mut self, apply: impl FnOnce(&mut SessionPicker)) {
        if let Layer::SessionPicker(picker) = &mut self.layer {
            apply(picker);
        }
    }

    fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        let current_id = self.session_id.clone();
        let others = sessions.into_iter().filter(|session| session.session_id != current_id).collect();
        let picker = SessionPicker::new(others, self.capabilities.session_preview);
        if let Some(id) = picker.initial_preview_request() {
            let _ = self.prompt_handle.session_preview(&SessionId::new(id));
        }
        self.open_layer(Layer::SessionPicker(picker));
    }

    /// A requested session has arrived: replay the updates that were buffered
    /// while it loaded. The transcript was cleared when the load was requested,
    /// so only per-turn state is reset here.
    fn on_session_loaded(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        let updates = self.session_loading_buffer.take(&session_id);
        self.session_id = session_id;
        self.config_options = config_options;
        self.reset_turn_state();
        self.transcript_generation = self.transcript_generation.wrapping_add(1);
        for update in updates {
            self.on_session_update(&update);
        }
        self.close_layer();
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session_loading_buffer.clear();
        self.close_elicitation_owner();
        self.close_layer();
        let previous_selections = extract_config_selections(&self.config_options);
        self.session_id = session_id;
        self.config_options = config_options;
        self.reset_conversation();
        self.notify("New session created");
        self.restore_config_selections(&previous_selections);
    }

    /// Server notifications feed three places: the status summary in the status
    /// line, the settings overlay's server pane, and whichever elicitation modal
    /// the notification may be completing.
    fn on_mcp_notification(&mut self, notification: &McpNotification) {
        if let McpNotification::ServerStatus { servers } = notification {
            self.server_statuses.clone_from(servers);
            self.unhealthy_server_count = servers
                .iter()
                .filter(|server| !matches!(server.status, acp_utils::notifications::McpServerStatus::Connected { .. }))
                .count();
            let servers = servers.clone();
            self.with_settings(|overlay| overlay.update_server_statuses(servers));
        }
        if let Layer::Elicitation(modal) = &mut self.layer
            && modal.on_notification(notification)
        {
            self.close_layer();
        }
        if let McpNotification::UrlElicitationComplete(params) = notification {
            self.with_settings(|overlay| overlay.on_url_elicitation_complete(params));
        }
    }

    /// The agent is gone: answer anything it is still waiting on, tear down
    /// every surface, and ask the event loop to exit.
    fn on_connection_closed(&mut self) {
        self.close_elicitation_owner();
        self.close_layer();
        self.close_layer();
        self.workspace_move_state = WorkspaceMoveState::Idle;
        self.session_loading_buffer.clear();
        self.surface_rect = None;
        self.pending_bell = None;
        self.exit_state = ExitState::Exiting;
    }

    /// Answers any elicitation the current overlay is holding, leaving the
    /// settings overlay itself open so its pane survives.
    fn close_elicitation_owner(&mut self) {
        match &mut self.layer {
            Layer::Elicitation(modal) => {
                modal.cancel();
                self.layer = Layer::None;
            }
            Layer::Settings(overlay) => overlay.cancel_pending_elicitation(),
            _ => {}
        }
    }

    fn on_session_update(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => {
                if let Some(text) = render_user_content_block(&chunk.content) {
                    self.transcript.push_user_message(&text);
                }
            }
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content {
                    self.transcript.append_text_chunk(&text_content.text);
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content {
                    self.transcript.append_thought_chunk(&text_content.text);
                }
            }
            acp::SessionUpdate::ToolCall(tool_call) => {
                self.transcript.close_thought_block();
                self.tool_calls.on_tool_call(tool_call);
                self.transcript.ensure_tool_segment(&tool_call.tool_call_id.0);
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                self.transcript.close_thought_block();
                self.tool_calls.on_tool_call_update(update);
                if self.tool_calls.has_tool(&update.tool_call_id.0) {
                    self.transcript.ensure_tool_segment(&update.tool_call_id.0);
                }
            }
            acp::SessionUpdate::AvailableCommandsUpdate(update) => {
                let agent_commands: Vec<_> = update
                    .available_commands
                    .iter()
                    .map(|command| CommandEntry {
                        name: command.name.clone(),
                        description: command.description.clone(),
                        has_input: command.input.is_some(),
                        hint: match &command.input {
                            Some(acp::AvailableCommandInput::Unstructured(input)) => Some(input.hint.clone()),
                            _ => None,
                        },
                        builtin: false,
                    })
                    .collect();
                self.available_commands = merge_builtins(agent_commands, &self.capabilities);
            }
            acp::SessionUpdate::ConfigOptionUpdate(update) => {
                self.config_options.clone_from(&update.config_options);
                self.transcript.close_thought_block();
                let options = self.config_options.clone();
                self.with_settings(|overlay| overlay.update_config_options(&options));
            }
            acp::SessionUpdate::Plan(plan) => {
                self.plan_tracker.replace(plan.entries.clone(), Instant::now());
                self.transcript.close_thought_block();
            }
            _ => {
                self.transcript.close_thought_block();
            }
        }
    }

    fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
        let was_in_flight = self.turn.prompt_in_flight;
        self.turn.prompt_in_flight = false;
        self.turn.compaction_active = false;
        self.tool_calls.finalize_running(terminal_status);
        self.transcript.close_thought_block();
        if was_in_flight && matches!(terminal_status, ToolStatus::Success) {
            self.pending_bell = Some(());
        }
    }
}

fn render_user_content_block(block: &acp::ContentBlock) -> Option<String> {
    match block {
        acp::ContentBlock::Text(text) => Some(text.text.clone()),
        acp::ContentBlock::Image(_) => Some("[image attachment]".to_string()),
        acp::ContentBlock::Audio(_) => Some("[audio attachment]".to_string()),
        _ => None,
    }
}

pub(super) fn plan_review_meta(
    params: &acp_utils::notifications::ElicitationParams,
) -> Option<utils::plan_review::PlanReviewElicitationMeta> {
    match &params.request {
        acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams { meta, .. } => {
            utils::plan_review::PlanReviewElicitationMeta::parse(meta.as_ref().map(|meta| &meta.0))
        }
        acp_utils::notifications::CreateElicitationRequestParams::UrlElicitationParams { .. } => None,
    }
}
