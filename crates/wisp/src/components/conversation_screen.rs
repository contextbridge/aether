use crate::components::app::PromptAttachment;
use crate::components::browser_authorization::{BrowserAuthorizationMessage, BrowserAuthorizationPrompt};
use crate::components::command_picker::CommandEntry;
use crate::components::conversation_window::{ConversationBuffer, ConversationWindow};
use crate::components::elicitation_form::{ElicitationForm, ElicitationMessage};
use crate::components::plan_tracker::PlanTracker;
use crate::components::plan_view::PlanView;
use crate::components::progress_indicator::{ProgressActivity, ProgressIndicator, WorkspaceProgress};
use crate::components::prompt_composer::{PromptComposer, PromptComposerMessage};
use crate::components::session_picker::{SessionEntry, SessionPicker, SessionPickerMessage};
use crate::components::tool_call_statuses::{PromptTermination, ToolCallStatuses};
use crate::components::workspace_picker::{WorkspacePicker, WorkspacePickerMessage};
use crate::keybindings::Keybindings;
use acp_utils::notifications::{
    AetherCapabilities, BrowserAuthorizationParams, BrowserAuthorizationResponseParams, ElicitationRequest,
    ElicitationResponse, PromptSearchParams, PromptSearchResponse, SessionPreviewResponse, WorkspaceEntry,
    WorkspaceMoveTarget,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{self as acp, SessionId};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;
use tui::{Component, Cursor, Event, Frame, Insets, ViewContext};

pub enum ConversationScreenMessage {
    SendPrompt { user_input: String, attachments: Vec<PromptAttachment> },
    ClearScreen,
    NewSession,
    OpenSettings,
    OpenSessionPicker,
    OpenWorkspacePicker,
    MoveWorkspace { target: WorkspaceMoveTarget },
    LoadSession { session_id: SessionId, cwd: PathBuf },
    SearchPrompts(PromptSearchParams),
    RequestSessionPreview { session_id: SessionId },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceMoveState {
    #[default]
    Idle,
    Listing,
    Picking,
    Moving,
    LoadingSession,
}

impl WorkspaceMoveState {
    fn progress(&self) -> WorkspaceProgress {
        match self {
            Self::Moving => WorkspaceProgress::Moving,
            Self::LoadingSession => WorkspaceProgress::LoadingSession,
            Self::Idle | Self::Listing | Self::Picking => WorkspaceProgress::None,
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

pub(crate) enum Modal {
    Elicitation(ElicitationForm),
    BrowserAuthorization(BrowserAuthorizationPrompt),
    SessionPicker(SessionPicker),
    WorkspacePicker(WorkspacePicker),
}

#[doc = include_str!("../docs/conversation_screen.md")]
pub struct ConversationScreen {
    pub(crate) conversation: ConversationBuffer,
    pub tool_call_statuses: ToolCallStatuses,
    pub(crate) prompt_composer: PromptComposer,
    pub(crate) plan_tracker: PlanTracker,
    pub(crate) progress_indicator: ProgressIndicator,
    pub(crate) workspace_move_state: WorkspaceMoveState,
    compaction_active: bool,
    waiting_for_response: bool,
    pub(crate) active_modal: Option<Modal>,
    pub(crate) content_padding: usize,
    pub(crate) pending_url_elicitations: HashSet<String>,
    capabilities: AetherCapabilities,
}

impl ConversationScreen {
    pub fn new(
        keybindings: Keybindings,
        content_padding: usize,
        working_dir: PathBuf,
        capabilities: AetherCapabilities,
    ) -> Self {
        Self {
            conversation: ConversationBuffer::new(),
            tool_call_statuses: ToolCallStatuses::new(),
            prompt_composer: PromptComposer::new(keybindings, working_dir, capabilities.clone()),
            plan_tracker: PlanTracker::default(),
            progress_indicator: ProgressIndicator::default(),
            workspace_move_state: WorkspaceMoveState::Idle,
            compaction_active: false,
            waiting_for_response: false,
            active_modal: None,
            content_padding,
            pending_url_elicitations: HashSet::new(),
            capabilities,
        }
    }

    pub fn has_modal(&self) -> bool {
        self.active_modal.is_some()
    }

    pub fn clear_prompt_composer(&mut self) {
        self.prompt_composer.clear_content();
    }

    pub fn set_working_dir(&mut self, working_dir: PathBuf) {
        self.prompt_composer.set_working_dir(working_dir);
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting_for_response
    }

    pub fn on_prompt_sent(&mut self) {
        self.waiting_for_response = true;
    }

    pub fn set_compaction_active(&mut self, active: bool) {
        self.compaction_active = active;
    }

    pub fn is_busy(&self) -> bool {
        self.waiting_for_response || self.tool_call_statuses.running_any()
    }

    pub fn wants_tick(&self) -> bool {
        self.is_busy()
            || self.compaction_active
            || self.workspace_move_state.progress() != WorkspaceProgress::None
            || self.plan_tracker_has_tick_driven_visibility()
    }

    pub fn on_tick(&mut self, now: Instant) {
        self.tool_call_statuses.on_tick(now);
        self.plan_tracker.on_tick(now);
        self.progress_indicator.on_tick();
    }

    pub fn refresh_caches(&mut self, _context: &ViewContext) {
        self.progress_indicator.update(ProgressActivity {
            agent_busy: self.is_busy(),
            workspace: self.workspace_move_state.progress(),
            compaction_active: self.compaction_active,
        });
        self.plan_tracker.cached_visible_entries();
    }

    pub fn reset_after_context_cleared(&mut self) {
        self.conversation.clear();
        self.tool_call_statuses.clear();
        self.waiting_for_response = false;
        self.workspace_move_state = WorkspaceMoveState::Idle;
        self.compaction_active = false;
        self.plan_tracker.clear();
        self.progress_indicator = ProgressIndicator::default();
        self.pending_url_elicitations.clear();
    }

    pub fn request_workspace_picker(&mut self) -> Result<ConversationScreenMessage, &'static str> {
        if self.waiting_for_response {
            return Err("Cannot move workspaces while a prompt is running.");
        }
        if !self.workspace_move_state.is_idle() {
            return Err("Cannot move workspaces while another workspace move is in progress.");
        }
        self.workspace_move_state = WorkspaceMoveState::Listing;
        self.prompt_composer.clear_content();
        Ok(ConversationScreenMessage::OpenWorkspacePicker)
    }

    pub fn open_workspace_picker(&mut self, workspaces: Vec<WorkspaceEntry>) {
        self.workspace_move_state = WorkspaceMoveState::Picking;
        self.active_modal = Some(Modal::WorkspacePicker(WorkspacePicker::new(workspaces)));
    }

    pub(crate) fn on_workspace_list_failed(&mut self, error: &str) {
        self.conversation.push_user_message(&format!("[wisp] Failed to list workspaces: {error}"));
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    pub(crate) fn on_workspace_move_started(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Moving;
    }

    pub(crate) fn on_workspace_session_loading(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::LoadingSession;
    }

    pub(crate) fn on_workspace_move_finished(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    pub(crate) fn on_workspace_move_failed(&mut self, error: &str) {
        self.conversation.push_user_message(&format!("[wisp] Workspace move failed: {error}"));
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    pub fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) -> Vec<ConversationScreenMessage> {
        let entries = sessions.into_iter().map(SessionEntry).collect();
        let mut picker = SessionPicker::new(entries, self.capabilities.session_preview);
        let messages = picker
            .initial_messages()
            .into_iter()
            .filter_map(|msg| match msg {
                SessionPickerMessage::RequestPreview { session_id } => {
                    Some(ConversationScreenMessage::RequestSessionPreview { session_id })
                }
                _ => None,
            })
            .collect();
        self.active_modal = Some(Modal::SessionPicker(picker));
        messages
    }

    pub fn on_session_update(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => {
                if let Some(text) = render_user_content_block(&chunk.content) {
                    self.conversation.push_user_message(&text);
                }
            }
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content {
                    self.conversation.append_text_chunk(&text_content.text);
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text_content) = &chunk.content {
                    self.conversation.append_thought_chunk(&text_content.text);
                }
            }
            acp::SessionUpdate::ToolCall(tool_call) => {
                self.conversation.close_thought_block();
                self.tool_call_statuses.on_tool_call(tool_call);
                self.conversation.ensure_tool_segment(&tool_call.tool_call_id.0);
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                self.conversation.close_thought_block();
                self.tool_call_statuses.on_tool_call_update(update);
                if self.tool_call_statuses.has_tool(&update.tool_call_id.0) {
                    self.conversation.ensure_tool_segment(&update.tool_call_id.0);
                }
            }
            acp::SessionUpdate::AvailableCommandsUpdate(update) => {
                let commands = update
                    .available_commands
                    .iter()
                    .map(|cmd| {
                        let hint = match cmd.input {
                            Some(acp::AvailableCommandInput::Unstructured(ref input)) => Some(input.hint.clone()),
                            _ => None,
                        };
                        CommandEntry {
                            name: cmd.name.clone(),
                            description: cmd.description.clone(),
                            has_input: cmd.input.is_some(),
                            hint,
                            builtin: false,
                        }
                    })
                    .collect();
                self.prompt_composer.set_available_commands(commands);
            }
            acp::SessionUpdate::Plan(plan) => {
                self.plan_tracker.replace(plan.entries.clone(), Instant::now());
            }
            _ => {
                self.conversation.close_thought_block();
            }
        }
    }

    pub fn on_prompt_done(&mut self, stop_reason: acp::StopReason) {
        self.on_prompt_terminated(&match stop_reason {
            acp::StopReason::Cancelled => PromptTermination::Cancelled,
            _ => PromptTermination::EndTurn,
        });
    }

    pub fn on_prompt_error(&mut self, error: &acp::Error) {
        tracing::error!("Prompt error: {error}");
        self.on_prompt_terminated(&PromptTermination::Failed(error.to_string()));
    }

    fn on_prompt_terminated(&mut self, termination: &PromptTermination) {
        self.waiting_for_response = false;
        self.compaction_active = false;
        self.tool_call_statuses.finalize_running(termination);
        self.conversation.close_thought_block();
    }

    pub fn reject_local_prompt(&mut self, message: &str) {
        self.waiting_for_response = false;
        self.compaction_active = false;
        self.conversation.push_user_message(&format!("[wisp] {message}"));
    }

    pub fn on_elicitation_request(
        &mut self,
        params: acp_utils::notifications::ElicitationParams,
        responder: Responder<ElicitationResponse>,
    ) {
        if matches!(&params.request, ElicitationRequest::Url(_)) {
            self.pending_url_elicitations.insert(params.server_name.clone());
        }
        self.active_modal = Some(Modal::Elicitation(ElicitationForm::from_params(params, responder)));
    }

    pub fn on_browser_authorization_request(
        &mut self,
        params: BrowserAuthorizationParams,
        responder: Responder<BrowserAuthorizationResponseParams>,
    ) {
        self.pending_url_elicitations.insert(params.server_name.clone());
        self.active_modal =
            Some(Modal::BrowserAuthorization(BrowserAuthorizationPrompt::from_params(params, responder)));
    }

    pub fn on_sub_agent_progress(&mut self, progress: &acp_utils::notifications::SubAgentProgressParams) {
        self.tool_call_statuses.on_sub_agent_progress(progress);
    }

    pub fn on_browser_authorization_completed(&mut self, server_name: &str) {
        if self.pending_url_elicitations.remove(server_name) {
            let modal_closed = match self.active_modal.as_mut() {
                Some(Modal::Elicitation(form)) => form.accept_browser_authorization_completed(server_name),
                Some(Modal::BrowserAuthorization(prompt)) => prompt.accept_completed(server_name),
                _ => false,
            };
            if modal_closed {
                self.active_modal = None;
            }
            self.conversation.push_user_message(&format!(
                "[wisp] {server_name} finished the browser flow. Retry the previous request if it did not resume automatically."
            ));
        }
    }

    fn plan_tracker_has_tick_driven_visibility(&self) -> bool {
        self.plan_tracker.has_completed_in_grace_period()
    }

    async fn handle_modal_key(&mut self, event: &Event) -> Option<Vec<ConversationScreenMessage>> {
        let modal = self.active_modal.as_mut()?;
        match modal {
            Modal::Elicitation(form) => {
                let outcome = form.on_event(event).await;
                for msg in outcome.unwrap_or_default() {
                    match msg {
                        ElicitationMessage::Responded => {
                            self.active_modal = None;
                        }
                        ElicitationMessage::UrlOpened { server_name } => {
                            self.pending_url_elicitations.insert(server_name.clone());
                            self.conversation.push_user_message(&format!(
                                "[wisp] Opened browser for {server_name}. Complete the flow, then retry the previous action if needed."
                            ));
                        }
                    }
                }
                Some(vec![])
            }
            Modal::BrowserAuthorization(prompt) => {
                let outcome = prompt.on_event(event).await;
                for msg in outcome.unwrap_or_default() {
                    match msg {
                        BrowserAuthorizationMessage::Responded => {
                            self.active_modal = None;
                        }
                        BrowserAuthorizationMessage::Opened { server_name } => {
                            self.pending_url_elicitations.insert(server_name.clone());
                            self.conversation.push_user_message(&format!(
                                "[wisp] Opened browser for {server_name}. Complete the flow, then retry the previous action if needed."
                            ));
                        }
                    }
                }
                Some(vec![])
            }
            Modal::SessionPicker(picker) => {
                let msgs = picker.on_event(event).await.unwrap_or_default();
                let mut out = Vec::new();
                for msg in msgs {
                    match msg {
                        SessionPickerMessage::Close => {
                            self.active_modal = None;
                        }
                        SessionPickerMessage::LoadSession { session_id, cwd } => {
                            self.active_modal = None;
                            self.reset_after_context_cleared();
                            out.push(ConversationScreenMessage::ClearScreen);
                            out.push(ConversationScreenMessage::LoadSession { session_id, cwd });
                        }
                        SessionPickerMessage::RequestPreview { session_id } => {
                            out.push(ConversationScreenMessage::RequestSessionPreview { session_id });
                        }
                    }
                }
                Some(out)
            }
            Modal::WorkspacePicker(picker) => {
                let msgs = picker.on_event(event).await.unwrap_or_default();
                let mut out = Vec::new();
                for msg in msgs {
                    match msg {
                        WorkspacePickerMessage::Close => {
                            self.active_modal = None;
                            self.workspace_move_state = WorkspaceMoveState::Idle;
                        }
                        WorkspacePickerMessage::Move { target } => {
                            self.active_modal = None;
                            out.push(ConversationScreenMessage::MoveWorkspace { target });
                        }
                    }
                }
                Some(out)
            }
        }
    }

    fn handle_prompt_composer_messages(
        &mut self,
        outcome: Option<Vec<PromptComposerMessage>>,
    ) -> Option<Vec<ConversationScreenMessage>> {
        let msgs = outcome?;
        let mut out = Vec::new();
        for msg in msgs {
            match msg {
                PromptComposerMessage::NewSession => {
                    self.reset_after_context_cleared();
                    out.push(ConversationScreenMessage::NewSession);
                }
                PromptComposerMessage::OpenSettings => {
                    out.push(ConversationScreenMessage::OpenSettings);
                }
                PromptComposerMessage::OpenSessionPicker => {
                    out.push(ConversationScreenMessage::OpenSessionPicker);
                }
                PromptComposerMessage::OpenWorkspacePicker => match self.request_workspace_picker() {
                    Ok(msg) => out.push(msg),
                    Err(message) => self.conversation.push_user_message(&format!("[wisp] {message}")),
                },
                PromptComposerMessage::SubmitRequested { user_input, attachments } => {
                    self.on_prompt_sent();
                    out.push(ConversationScreenMessage::SendPrompt { user_input, attachments });
                }
                PromptComposerMessage::SearchPrompts(params) => {
                    out.push(ConversationScreenMessage::SearchPrompts(params));
                }
            }
        }
        Some(out)
    }

    pub fn on_prompt_search_results(&mut self, response: PromptSearchResponse) {
        self.prompt_composer.on_prompt_search_results(response);
    }

    pub fn on_prompt_search_failed(&mut self, query: &str, error: String) {
        self.prompt_composer.on_prompt_search_failed(query, error);
    }

    pub fn on_session_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        if let Some(Modal::SessionPicker(picker)) = self.active_modal.as_mut() {
            picker.on_preview_loaded(preview);
        }
    }

    pub fn on_session_preview_failed(&mut self, session_id: &str, error: String) {
        if let Some(Modal::SessionPicker(picker)) = self.active_modal.as_mut() {
            picker.on_preview_failed(session_id, error);
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

impl Component for ConversationScreen {
    type Message = ConversationScreenMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<ConversationScreenMessage>> {
        if self.active_modal.is_some() {
            return self.handle_modal_key(event).await;
        }

        let composer_outcome = self.prompt_composer.on_event(event).await;
        if composer_outcome.is_some() {
            return self.handle_prompt_composer_messages(composer_outcome);
        }

        None
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let conversation_window = ConversationWindow {
            conversation: &self.conversation,
            tool_call_statuses: &self.tool_call_statuses,
            content_padding: self.content_padding,
        };
        let plan_view = PlanView { entries: self.plan_tracker.cached_entries() };

        let pad_u16 = u16::try_from(self.content_padding).unwrap_or(u16::MAX);
        let content_ctx = ctx.inset(Insets::horizontal(pad_u16));
        let modal_active = self.active_modal.is_some();

        let conversation_frame = conversation_window.render(ctx);
        let plan_frame = plan_view.render(&content_ctx).indent(pad_u16);
        let progress_frame = self.progress_indicator.render(&content_ctx).indent(pad_u16);

        let mut prompt_frame = self.prompt_composer.render(ctx);
        if modal_active {
            prompt_frame = prompt_frame.with_cursor(Cursor::hidden());
        }

        let modal_frame = match &mut self.active_modal {
            Some(Modal::SessionPicker(picker)) => Some(picker.render(ctx)),
            Some(Modal::WorkspacePicker(picker)) => Some(picker.render(ctx)),
            Some(Modal::Elicitation(form)) => Some(form.render(ctx)),
            Some(Modal::BrowserAuthorization(prompt)) => Some(prompt.render(ctx)),
            None => None,
        };

        let mut sections = vec![conversation_frame, plan_frame, progress_frame, prompt_frame];
        if let Some(frame) = modal_frame {
            sections.push(frame);
        }
        Frame::vstack(sections)
    }
}
