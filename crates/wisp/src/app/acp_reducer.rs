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
use acp_utils::client::{AcpEvent, LoadedSession};
use acp_utils::notifications::McpNotification;
use agent_client_protocol::schema::v1::{self as acp, CreateElicitationRequest, ElicitationMode, SessionId};
use std::time::Instant;

impl App {
    #[allow(clippy::too_many_lines)]
    pub fn on_acp_event(&mut self, event: AcpEvent) {
        match event {
            AcpEvent::SessionUpdate { session_id, update } => {
                if &session_id == self.session.session_id() {
                    self.on_session_update(&update);
                }
            }
            AcpEvent::PromptDone(stop_reason) => {
                let status = match stop_reason {
                    acp::StopReason::Cancelled => ToolStatus::Error("cancelled".to_string()),
                    _ => ToolStatus::Success,
                };
                self.finish_prompt(&status);
            }
            AcpEvent::PromptError(error) => self.finish_prompt(&ToolStatus::Error(error.to_string())),
            AcpEvent::SessionsListed { .. } => {}
            AcpEvent::ContextCompaction(params) => {
                self.conversation.turn_mut().set_compaction_active(params.active);
            }
            AcpEvent::ContextCleared(_) => {
                self.reset_conversation();
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                let params = *params;
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
                if let Some(modal) = ElicitationModal::with_url_handlers(
                    params,
                    responder,
                    self.browser_opener.clone(),
                    self.clipboard_writer.clone(),
                ) {
                    self.open_overlay(Overlay::Elicitation(modal));
                }
            }
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(&notification),
            AcpEvent::AuthMethodsUpdated(params) => {
                self.session.set_auth_methods(&params.auth_methods);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_auth_methods(&params.auth_methods);
                }
            }
            AcpEvent::ConnectionClosed => self.on_connection_closed(),
            AcpEvent::SubAgentProgress(progress) => {
                self.conversation.on_sub_agent_progress(&progress);
            }
        }
    }

    /// Reports why a workspace move could not proceed and leaves move mode.
    pub(super) fn abandon_workspace_move(&mut self, message: &str) {
        self.notify(message);
        self.session.end_workspace_move();
    }

    pub(super) fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        let current_id = self.session.session_id().clone();
        let others = sessions.into_iter().filter(|session| session.session_id != current_id).collect();
        let picker = SessionPicker::new(others, self.session.capabilities().session_preview);
        if let Some(id) = picker.initial_preview_request() {
            self.queue(Command::Agent(AgentCommand::SessionPreview { session_id: id }));
        }
        self.open_overlay(Overlay::Sessions(picker));
    }

    pub(super) fn on_loaded_session(&mut self, loaded: LoadedSession) {
        let LoadedSession { session_id, response, replay } = loaded;
        self.reset_turn_state();
        for notification in replay {
            self.on_session_update(&notification.update);
        }
        self.session.set_session(session_id, response.config_options.unwrap_or_default());
        self.return_to_conversation();
        self.session.end_workspace_move();
    }

    pub(super) fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
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
            acp::SessionUpdate::UsageUpdate(usage) => {
                self.conversation.turn_mut().set_context_usage(Some(ContextUsageDisplay {
                    used_tokens: u32::try_from(usage.used).unwrap_or(u32::MAX),
                    limit_tokens: u32::try_from(usage.size).unwrap_or(u32::MAX),
                }));
            }
            _ => {
                self.conversation.finish_current_block();
            }
        }
    }

    pub(super) fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
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
    params: &CreateElicitationRequest,
) -> Option<utils::plan_review::PlanReviewElicitationMeta> {
    if !matches!(params.mode, ElicitationMode::Form(_)) {
        return None;
    }
    utils::plan_review::PlanReviewElicitationMeta::parse(params.meta.as_ref())
}
