use super::session::builtin_commands;
use super::{App, ExitState, ForegroundOperation, Overlay, PromptPhase, Route};
use crate::command::{AgentCommand, Command, TerminalCommand};
use crate::conversation::tool_calls::ToolStatus;
use crate::conversation::{ContextUsageDisplay, MessageRole};
use crate::screens::artifact_review::ArtifactReviewScreen;
use crate::surfaces::modal::ElicitationModal;
use crate::surfaces::picker::CommandEntry;
use crate::surfaces::session_picker::SessionPicker;
use acp_utils::client::AcpEvent;
use acp_utils::notifications::McpNotification;
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{
    self as acp, CreateElicitationRequest, ElicitationMode, SessionId, SessionUpdate, StateUpdate,
};
use std::time::Instant;
use utils::artifact_review::ArtifactReviewElicitationMeta;

impl App {
    #[allow(clippy::too_many_lines)]
    pub fn on_acp_event(&mut self, event: AcpEvent) {
        match event {
            AcpEvent::SessionUpdate(notification) => {
                if &notification.session_id == self.session.session_id()
                    || matches!(self.foreground, ForegroundOperation::CreatingSession { .. })
                {
                    self.on_session_update(&notification.update);
                }
            }
            AcpEvent::ContextCleared(_) => {
                self.reset_conversation();
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                let params = *params;
                self.close_elicitation_owner();
                if let Some(meta) = artifact_review_meta(&params) {
                    self.open_route(Route::ArtifactReview(Box::new(ArtifactReviewScreen::new(meta, responder))));
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
                if self.conversation.progress_indicator().accepts_activity() {
                    self.conversation.on_sub_agent_progress(&progress);
                }
            }
        }
    }

    /// Reports why a workspace move could not proceed and leaves move mode.
    pub(super) fn abandon_workspace_move(&mut self, message: &str) {
        self.notify(message);
        self.foreground = ForegroundOperation::Idle;
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

    pub(super) fn on_resumed_session(&mut self, session_id: &SessionId, response: acp::ResumeSessionResponse) {
        match &self.foreground {
            ForegroundOperation::ResumingSession { session_id: expected, cwd }
            | ForegroundOperation::LoadingWorkspaceSession { session_id: expected, cwd }
                if expected == session_id =>
            {
                if self.session.working_dir() != cwd {
                    let cwd = cwd.clone();
                    self.session.set_working_dir(cwd.clone());
                    self.resolve_workspace(cwd);
                }
            }
            _ => return,
        }
        self.session.update_config_options(response.config_options);
        if matches!(self.foreground, ForegroundOperation::LoadingWorkspaceSession { .. }) {
            self.notify(&format!("Moved to {}", self.workspace_display_path(self.session.working_dir())));
        }
        self.return_to_conversation();
        self.foreground = ForegroundOperation::Idle;
    }

    pub(super) fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        if !matches!(self.foreground, ForegroundOperation::Idle | ForegroundOperation::CreatingSession { .. }) {
            return;
        }
        let previous_selections = match std::mem::take(&mut self.foreground) {
            ForegroundOperation::CreatingSession { previous_selections } => previous_selections,
            _ => Vec::new(),
        };
        self.close_elicitation_owner();
        self.return_to_conversation();
        self.session.set_session(session_id, config_options);
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

    /// The connection is gone, but a remote agent may still be running.
    /// Release local interactions and overlays, then ask the event loop to exit.
    fn on_connection_closed(&mut self) {
        self.close_elicitation_owner();
        self.return_to_conversation();
        self.foreground = ForegroundOperation::Idle;
        self.commands.retain(|command| !matches!(command, Command::Terminal(TerminalCommand::RingBell)));
        self.exit_state = ExitState::ConnectionLost;
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

    fn on_session_update(&mut self, update: &SessionUpdate) {
        if matches!(update, SessionUpdate::StateUpdate(StateUpdate::Running(_))) && self.foreground.is_idle() {
            self.foreground = ForegroundOperation::Prompt(PromptPhase::Running);
            self.conversation.progress_indicator_mut().prompt_started();
        }
        if self.waiting_for_response() {
            self.observe_activity(update);
        }
        match update {
            SessionUpdate::CompactionUpdate(update) => {
                if self.conversation.progress_indicator().accepts_activity() {
                    self.conversation.turn_mut().apply_compaction(update);
                }
            }
            SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) if self.waiting_for_response() => {
                let status = match idle.stop_reason {
                    Some(acp::StopReason::Cancelled) => ToolStatus::Error("cancelled".to_string()),
                    _ => ToolStatus::Success,
                };
                self.finish_prompt(&status);
            }
            SessionUpdate::UserMessage(message) => {
                self.conversation.upsert_message(MessageRole::User, message.message_id.clone(), &message.content);
            }
            SessionUpdate::AgentMessage(message) => {
                self.conversation.upsert_message(MessageRole::Assistant, message.message_id.clone(), &message.content);
            }
            SessionUpdate::UserMessageChunk(chunk) => {
                self.conversation.append_message_chunk(MessageRole::User, chunk);
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.conversation.append_message_chunk(MessageRole::Assistant, chunk);
            }
            SessionUpdate::ToolCallContentChunk(chunk) => {
                self.conversation.on_tool_call_content_chunk(chunk);
            }
            SessionUpdate::ToolCallUpdate(update) => {
                self.conversation.on_tool_call_update(update);
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                let agent_commands: Vec<_> = update
                    .available_commands
                    .iter()
                    .map(|command| CommandEntry {
                        name: command.name.clone(),
                        description: command.description.clone(),
                        has_input: command.input.is_some(),
                        hint: match &command.input {
                            Some(acp::AvailableCommandInput::Text(input)) => Some(input.hint.clone()),
                            _ => None,
                        },
                        builtin: false,
                    })
                    .collect();
                let mut all = builtin_commands(self.session.capabilities());
                all.extend(agent_commands);
                self.available_commands = all;
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.session.update_config_options(update.config_options.clone());
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_config_options(self.session.config_options());
                }
            }
            SessionUpdate::PlanUpdate(plan) => {
                self.conversation.plan_tracker_mut().apply_update(plan, Instant::now());
            }
            SessionUpdate::UsageUpdate(usage) => {
                self.conversation.turn_mut().set_context_usage(Some(ContextUsageDisplay {
                    used_tokens: u32::try_from(usage.used).unwrap_or(u32::MAX),
                    limit_tokens: u32::try_from(usage.size).unwrap_or(u32::MAX),
                }));
            }
            _ => {}
        }
    }

    fn observe_activity(&mut self, update: &SessionUpdate) {
        let indicator = self.conversation.progress_indicator_mut();
        match update {
            SessionUpdate::AgentMessageChunk(_) | SessionUpdate::StateUpdate(StateUpdate::Running(_)) => {
                indicator.response_started();
            }
            SessionUpdate::StateUpdate(StateUpdate::RequiresAction(_)) => indicator.requires_action(),
            SessionUpdate::ToolCallUpdate(_) => indicator.tool_activity(),
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text) = &chunk.content
                    && !text.text.is_empty()
                {
                    indicator.record_thought(&chunk.message_id, &text.text);
                }
            }
            SessionUpdate::AgentThought(message) => match &message.content {
                MaybeUndefined::Undefined => {}
                MaybeUndefined::Null => indicator.replace_thought(&message.message_id, ""),
                MaybeUndefined::Value(blocks) => {
                    let text = acp_utils::content::map_content_blocks_to_text(blocks.clone());
                    indicator.replace_thought(&message.message_id, &text);
                }
            },
            _ => {}
        }
    }

    pub(super) fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
        let was_in_flight = self.waiting_for_response();
        self.foreground.finish_prompt();
        self.conversation.turn_mut().clear_compactions();
        self.conversation.progress_indicator_mut().prompt_finished();
        self.conversation.finish_turn(terminal_status);
        if was_in_flight && matches!(terminal_status, ToolStatus::Success) {
            self.queue(Command::Terminal(TerminalCommand::RingBell));
        }
    }
}

pub(super) fn artifact_review_meta(params: &CreateElicitationRequest) -> Option<ArtifactReviewElicitationMeta> {
    if !matches!(params.mode, ElicitationMode::Form(_)) {
        return None;
    }
    ArtifactReviewElicitationMeta::parse(params.meta.as_ref())
}
