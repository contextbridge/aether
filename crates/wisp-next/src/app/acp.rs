use super::config::extract_config_selections;
use super::session::merge_builtins;
use super::{
    AcpEvent, App, CommandEntry, ContextUsageDisplay, ElicitationModal, ExitState, Instant, McpNotification,
    ModalOutcome, OverlayLayer, SessionId, SessionPicker, ToolStatus, WorkspaceMoveState, WorkspacePicker, acp,
    render_user_content_block,
};

impl App {
    #[allow(clippy::too_many_lines)]
    pub fn on_acp_event(&mut self, event: AcpEvent) {
        self.on_acp_event_inner(event);
        self.refresh_progress();
    }

    #[allow(clippy::too_many_lines)]
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
                self.transcript.push_user_message(&format!("[wisp-next] Prompt failed: {error}"));
            }
            AcpEvent::ContextUsage(params) => {
                self.context_usage =
                    params.usage.context_limit.map(|limit| ContextUsageDisplay::new(params.usage.input_tokens, limit));
            }
            AcpEvent::ContextCompaction(params) => {
                self.compaction_active = params.active;
            }
            AcpEvent::ContextCleared(_) => {
                self.transcript.clear();
                self.tool_calls.clear();
                self.plan_tracker.clear();
                self.prompt_in_flight = false;
                self.submitted_prompt_count = 0;
                self.compaction_active = false;
                self.context_usage = None;
                self.last_drained_kind = None;
                self.progress_indicator.reset();
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.transcript.push_user_message("[wisp-next] Context cleared");
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                match std::mem::take(&mut self.overlay) {
                    OverlayLayer::Elicitation(mut modal) => modal.cancel(),
                    OverlayLayer::Settings(o) => self.overlay = OverlayLayer::Settings(o),
                    _ => {}
                }
                if let Some(meta) = plan_review_meta(&params) {
                    self.screen_router.open_plan_review(meta, responder);
                    return;
                }
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.on_elicitation_request(params, responder);
                    return;
                }
                self.overlay = OverlayLayer::Elicitation(ElicitationModal::new(params, responder));
            }
            AcpEvent::McpNotification(notification) => {
                if let McpNotification::ServerStatus { ref servers } = notification {
                    self.server_statuses.clone_from(servers);
                    self.unhealthy_server_count = servers
                        .iter()
                        .filter(|s| !matches!(s.status, acp_utils::notifications::McpServerStatus::Connected { .. }))
                        .count();
                    if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                        overlay.update_server_statuses(servers.clone());
                    }
                }
                if let OverlayLayer::Elicitation(modal) = &mut self.overlay
                    && matches!(modal.on_notification(&notification), ModalOutcome::Close)
                {
                    self.overlay = OverlayLayer::None;
                }
                if let McpNotification::UrlElicitationComplete(ref params) = notification
                    && let OverlayLayer::Settings(overlay) = &mut self.overlay
                {
                    overlay.on_url_elicitation_complete(params);
                }
            }
            AcpEvent::AuthMethodsUpdated(params) => {
                self.auth_methods.clone_from(&params.auth_methods);
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.update_auth_methods(params.auth_methods);
                }
            }
            AcpEvent::AuthenticateComplete { method_id } => {
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.on_authenticate_complete(&method_id);
                }
            }
            AcpEvent::AuthenticateFailed { method_id, error } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.on_authenticate_failed(&method_id);
                }
            }
            AcpEvent::ConnectionClosed => {
                match std::mem::take(&mut self.overlay) {
                    OverlayLayer::Elicitation(mut modal) => modal.cancel(),
                    OverlayLayer::Settings(mut overlay) => overlay.cancel_pending_elicitation(),
                    _ => {}
                }
                self.workspace_move_state = WorkspaceMoveState::Idle;
                self.session_loading_buffer.clear();
                self.screen_router.close();
                self.surface_rect = None;
                self.pending_bell = None;
                self.exit_state = ExitState::Exiting;
            }
            AcpEvent::ConfigOptionUpdateFailed { error } => {
                tracing::warn!("set_session_config_option failed: {error}");
                self.transcript.push_user_message(&format!("[wisp-next] Failed to update setting: {error}"));
            }
            AcpEvent::SessionsListed { sessions } => {
                let current_id = self.session_id.clone();
                let filtered: Vec<_> = sessions.into_iter().filter(|s| s.session_id != current_id).collect();
                let preview_enabled = self.capabilities.session_preview;
                let picker = SessionPicker::new(filtered, preview_enabled);
                if let Some(id) = picker.initial_preview_request() {
                    let _ = self.prompt_handle.session_preview(&SessionId::new(id));
                }
                self.overlay = OverlayLayer::SessionPicker(picker);
            }
            AcpEvent::SessionLoaded { session_id, config_options } => {
                let updates = self.session_loading_buffer.take(&session_id);
                self.session_id = session_id.clone();
                self.config_options = config_options;
                self.plan_tracker.clear();
                self.compaction_active = false;
                self.progress_indicator.reset();
                for update in updates {
                    self.on_session_update(&update);
                }
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.overlay = OverlayLayer::None;
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            AcpEvent::NewSessionCreated { session_id, config_options } => {
                self.session_loading_buffer.clear();
                match std::mem::take(&mut self.overlay) {
                    OverlayLayer::Settings(mut overlay) => overlay.cancel_pending_elicitation(),
                    OverlayLayer::Elicitation(mut modal) => modal.cancel(),
                    _ => {}
                }
                let previous_selections = extract_config_selections(&self.config_options);
                self.session_id = session_id;
                self.config_options = config_options;
                self.transcript.clear();
                self.tool_calls.clear();
                self.plan_tracker.clear();
                self.prompt_in_flight = false;
                self.submitted_prompt_count = 0;
                self.compaction_active = false;
                self.context_usage = None;
                self.last_drained_kind = None;
                self.progress_indicator.reset();
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.transcript.push_user_message("[wisp-next] New session created");
                self.restore_config_selections(&previous_selections);
            }
            AcpEvent::SessionPreviewLoaded(preview) => {
                if let OverlayLayer::SessionPicker(picker) = &mut self.overlay {
                    picker.on_preview_loaded(preview);
                }
            }
            AcpEvent::SessionPreviewFailed { session_id, error } => {
                if let OverlayLayer::SessionPicker(picker) = &mut self.overlay {
                    picker.on_preview_failed(&session_id, error);
                }
            }
            AcpEvent::PromptSearchResults(response) => {
                self.composer.prompt_search_on_results(response);
            }
            AcpEvent::PromptSearchFailed { query: _, search_generation, error } => {
                self.composer.prompt_search_on_failed(search_generation, error);
            }
            AcpEvent::WorkspacesListed(response) => {
                let picker = WorkspacePicker::new(response.workspaces);
                self.overlay = OverlayLayer::WorkspacePicker(picker);
                self.workspace_move_state = WorkspaceMoveState::Picking;
            }
            AcpEvent::WorkspaceListFailed { error } => {
                self.transcript.push_user_message(&format!("[wisp-next] Failed to list workspaces: {error}"));
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            AcpEvent::WorkspaceMoved(response) => {
                self.on_workspace_moved(response.new_cwd);
            }
            AcpEvent::WorkspaceMoveFailed { error } => {
                self.transcript.push_user_message(&format!("[wisp-next] Workspace move failed: {error}"));
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            AcpEvent::SubAgentProgress(progress) => {
                self.tool_calls.on_sub_agent_progress(&progress);
                if self.tool_calls.has_tool(&progress.parent_tool_id) {
                    self.transcript.ensure_tool_segment(&progress.parent_tool_id);
                }
            }
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
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.update_config_options(&self.config_options);
                }
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
        let was_in_flight = self.prompt_in_flight;
        self.prompt_in_flight = false;
        self.compaction_active = false;
        self.tool_calls.finalize_running(terminal_status);
        self.transcript.close_thought_block();
        if was_in_flight && matches!(terminal_status, ToolStatus::Success) {
            self.pending_bell = Some(());
        }
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
