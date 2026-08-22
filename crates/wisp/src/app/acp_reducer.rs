use super::session::builtin_commands;
use super::{App, ExitState, Overlay, Route};
use crate::attachment::placeholder_for_content_block;
use crate::command::{AgentCommand, Command, TerminalCommand};
use crate::conversation::ContextUsageDisplay;
use crate::conversation::tool_calls::ToolStatus;
use crate::screens::plan_review::PlanReviewScreen;
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::picker::CommandEntry;
use crate::surfaces::session_picker::SessionPicker;
use crate::surfaces::workspace_picker::WorkspacePicker;
use acp_utils::client::AcpEvent;
use acp_utils::notifications::McpNotification;
use agent_client_protocol::schema::v1::{self as acp, SessionId};
use std::time::Instant;

impl App {
    #[allow(clippy::too_many_lines)]
    pub fn on_acp_event(&mut self, event: AcpEvent) {
        match event {
            AcpEvent::SessionUpdate { session_id, update } => {
                // An update that is neither buffered for a pending load nor
                // addressed to the session on screen belongs to one the user
                // has already moved on from.
                if let Some(passthrough) = self.session.buffer_update(&session_id, *update)
                    && &session_id == self.session.session_id()
                {
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
                self.session.clear_loads();
                self.session.abandon_workspace_load();
                self.finish_prompt(&ToolStatus::Error(format!("failed: {error}")));
                self.notify(&format!("Prompt failed: {error}"));
            }
            AcpEvent::ContextUsage(params) => {
                self.conversation.turn_mut().set_context_usage(
                    params.usage.context_limit.map(|limit| ContextUsageDisplay {
                        used_tokens: params.usage.input_tokens,
                        limit_tokens: limit,
                    }),
                );
            }
            AcpEvent::ContextCompaction(params) => {
                self.conversation.turn_mut().set_compaction_active(params.active);
            }
            AcpEvent::ContextCleared(_) => {
                self.reset_conversation();
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                self.close_elicitation_owner();
                if let Some(meta) = plan_review_meta(&params) {
                    self.open_route(Route::PlanReview(Box::new(PlanReviewScreen::new(meta, responder))));
                    return;
                }
                // The settings overlay answers its own elicitations in place so
                // an OAuth prompt does not tear down the pane that started it.
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_elicitation_request(
                        params,
                        responder,
                        self.browser_opener.clone(),
                        self.clipboard_writer.clone(),
                    );
                    return;
                }
                self.open_overlay(Overlay::Elicitation(ElicitationModal::with_url_handlers(
                    params,
                    responder,
                    self.browser_opener.clone(),
                    self.clipboard_writer.clone(),
                )));
            }
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(&notification),
            AcpEvent::AuthMethodsUpdated(params) => {
                self.session.set_auth_methods(&params.auth_methods);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_auth_methods(&params.auth_methods);
                }
            }
            AcpEvent::AuthenticateComplete { method_id } => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_complete(&method_id);
                }
            }
            AcpEvent::AuthenticateFailed { method_id, error } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_failed(&method_id);
                }
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
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_loaded(preview);
                }
            }
            AcpEvent::SessionPreviewFailed { session_id, error } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_failed(&session_id, error);
                }
            }
            AcpEvent::PromptSearchResults(response) => {
                self.composer.prompt_search_on_results(response);
            }
            AcpEvent::PromptSearchFailed { query, error } => {
                if let Some(picker) = self.composer.prompt_search_mut() {
                    picker.on_failed(&query, error);
                }
            }
            AcpEvent::WorkspacesListed(response) => {
                self.open_overlay(Overlay::Workspaces(WorkspacePicker::new(response.workspaces)));
                self.session.begin_workspace_picking();
            }
            AcpEvent::WorkspaceMoved(response) => self.on_workspace_moved(response.new_cwd),
            AcpEvent::WorkspaceListFailed { error } => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            AcpEvent::WorkspaceMoveFailed { error } => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            AcpEvent::SubAgentProgress(progress) => {
                self.conversation.on_sub_agent_progress(&progress);
            }
        }
    }

    /// Reports why a workspace move could not proceed and leaves move mode.
    fn abandon_workspace_move(&mut self, message: &str) {
        self.notify(message);
        self.session.end_workspace_move();
    }

    fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        let current_id = self.session.session_id().clone();
        let others = sessions.into_iter().filter(|session| session.session_id != current_id).collect();
        let picker = SessionPicker::new(others, self.session.capabilities().session_preview);
        if let Some(id) = picker.initial_preview_request() {
            self.queue(Command::Agent(AgentCommand::SessionPreview { session_id: id }));
        }
        self.open_overlay(Overlay::Sessions(picker));
    }

    /// A requested session has arrived: replay the updates that were buffered
    /// while it loaded. The conversation was cleared when the load was requested,
    /// so only per-turn state is reset here.
    fn on_session_loaded(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        let updates = self.session.take_buffered_updates(&session_id);
        self.session.set_session(session_id, config_options);
        self.reset_turn_state();
        for update in updates {
            self.on_session_update(&update);
        }
        self.return_to_conversation();
        self.session.end_workspace_move();
    }

    fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session.clear_loads();
        self.close_elicitation_owner();
        self.return_to_conversation();
        let previous_selections: Vec<(String, String)> = self
            .session
            .config_options()
            .iter()
            .filter_map(|option| option.select().map(|select| (option.id.clone(), select.current_value.to_string())))
            .collect();
        self.session.set_session(session_id, config_options);
        self.reset_conversation();
        self.restore_config_selections(&previous_selections);
    }

    /// Server notifications feed the status summary in the status line and settings overlay.
    fn on_mcp_notification(&mut self, notification: &McpNotification) {
        let McpNotification::ServerStatus { servers } = notification;
        self.session.update_server_statuses(servers);
        let servers = servers.clone();
        if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.update_server_statuses(servers);
        }
    }

    /// The agent is gone: answer anything it is still waiting on, tear down
    /// every route and overlay, and ask the event loop to exit.
    fn on_connection_closed(&mut self) {
        self.close_elicitation_owner();
        self.return_to_conversation();
        self.session.end_workspace_move();
        self.session.clear_loads();
        self.commands.retain(|command| !matches!(command, Command::Terminal(TerminalCommand::RingBell)));
        self.exit_state = ExitState::Exiting;
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

    fn on_session_update(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => {
                if let Some(text) = match &chunk.content {
                    acp::ContentBlock::Text(text) => Some(text.text.clone()),
                    block => placeholder_for_content_block(block).map(str::to_string),
                } {
                    self.conversation.append_user_content(text);
                }
            }
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content {
                    if !text_content.text.is_empty() {
                        self.conversation.progress_indicator_mut().response_started();
                    }
                    self.conversation.append_assistant_chunk(&text_content.text);
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content
                    && !text_content.text.is_empty()
                {
                    self.conversation.progress_indicator_mut().record_thought(&text_content.text);
                }
            }
            acp::SessionUpdate::ToolCall(tool_call) => {
                self.conversation.progress_indicator_mut().tool_activity();
                self.conversation.on_tool_call(tool_call);
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                self.conversation.progress_indicator_mut().tool_activity();
                self.conversation.on_tool_call_update(update);
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
                let mut all = builtin_commands(self.session.capabilities());
                all.extend(agent_commands);
                self.available_commands = all;
            }
            acp::SessionUpdate::ConfigOptionUpdate(update) => {
                self.session.update_config_options(update.config_options.clone());
                self.conversation.finish_current_block();
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_config_options(self.session.config_options());
                }
            }
            acp::SessionUpdate::Plan(plan) => {
                self.conversation.plan_tracker_mut().replace(plan.entries.clone(), Instant::now());
                self.conversation.finish_current_block();
            }
            _ => {
                self.conversation.finish_current_block();
            }
        }
    }

    fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
        let was_in_flight = self.waiting_for_response();
        self.conversation.turn_mut().set_prompt_in_flight(false);
        self.conversation.turn_mut().set_compaction_active(false);
        self.conversation.progress_indicator_mut().prompt_finished();
        self.conversation.finish_turn(terminal_status);
        if was_in_flight && matches!(terminal_status, ToolStatus::Success) {
            self.queue(Command::Terminal(TerminalCommand::RingBell));
        }
    }
}

pub(super) fn plan_review_meta(
    params: &acp_utils::notifications::ElicitationParams,
) -> Option<utils::plan_review::PlanReviewElicitationMeta> {
    match &params.request {
        acp_utils::notifications::ElicitRequestParams::FormElicitationParams { meta, .. } => {
            utils::plan_review::PlanReviewElicitationMeta::parse(meta.as_ref().map(|meta| &**meta).map(|meta| &**meta))
        }
        _ => None,
    }
}
