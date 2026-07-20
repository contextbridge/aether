use crate::composer::Composer;
use crate::diff::DiffPreview;
use crate::keybindings::Keybindings;
use crate::modal::{ElicitationModal, ModalOutcome};
use crate::picker::CommandEntry;
use crate::screen_router::{ScreenEffect, ScreenEvent, ScreenRouter};
use crate::settings::{UiSettings, resolve_content_padding};
use crate::tool_calls::{ToolCallEntry, ToolCallLog, ToolStatus};
use crate::transcript::{SegmentContent, Transcript};
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle};
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Root UI state: reduces terminal input and ACP events into the transcript,
/// tool-call log, and composer that the renderer draws each frame.
pub struct App {
    session_id: SessionId,
    agent_name: String,
    prompt_handle: AcpPromptHandle,
    keybindings: Keybindings,
    workspace_status: WorkspaceStatus,
    content_padding: usize,
    working_dir: PathBuf,
    available_commands: Vec<CommandEntry>,
    transcript: Transcript,
    tool_calls: ToolCallLog,
    composer: Composer,
    prompt_in_flight: bool,
    context_percent: Option<u8>,
    ctrl_c_armed_at: Option<Instant>,
    exit_requested: bool,
    spinner_tick: usize,
    last_drained_kind: Option<HistoryKind>,
    transcript_generation: u64,
    modal: Option<ElicitationModal>,
    screen_router: ScreenRouter,
    pending_screen_effects: std::collections::VecDeque<ScreenEffect>,
}

pub struct AppConfig {
    pub session_id: SessionId,
    pub agent_name: String,
    pub workspace_status: WorkspaceStatus,
    pub prompt_handle: AcpPromptHandle,
    pub working_dir: PathBuf,
    pub settings: UiSettings,
}

/// A transcript segment resolved for rendering: tool calls carry their final
/// title and status instead of an id that needs a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    User(String),
    Text(String),
    Thought(String),
    Tool { title: String, status: ToolStatus, diff: Option<DiffPreview> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryKind {
    User,
    Text,
    Thought,
    Tool,
}

impl HistoryItem {
    pub fn kind(&self) -> HistoryKind {
        match self {
            HistoryItem::User(_) => HistoryKind::User,
            HistoryItem::Text(_) => HistoryKind::Text,
            HistoryItem::Thought(_) => HistoryKind::Thought,
            HistoryItem::Tool { .. } => HistoryKind::Tool,
        }
    }
}

const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    pub fn new(config: AppConfig) -> Self {
        let content_padding = resolve_content_padding(&config.settings);
        Self {
            session_id: config.session_id,
            agent_name: config.agent_name,
            prompt_handle: config.prompt_handle,
            keybindings: Keybindings::default(),
            workspace_status: config.workspace_status,
            content_padding,
            working_dir: config.working_dir,
            available_commands: Vec::new(),
            transcript: Transcript::new(),
            tool_calls: ToolCallLog::new(),
            composer: Composer::new(),
            prompt_in_flight: false,
            context_percent: None,
            ctrl_c_armed_at: None,
            exit_requested: false,
            spinner_tick: 0,
            last_drained_kind: None,
            transcript_generation: 0,
            modal: None,
            screen_router: ScreenRouter::new(),
            pending_screen_effects: std::collections::VecDeque::new(),
        }
    }

    pub fn on_acp_event(&mut self, event: AcpEvent) {
        match event {
            AcpEvent::SessionUpdate { update, .. } => self.on_session_update(&update),
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
                self.context_percent = params.usage.context_limit.filter(|limit| *limit > 0).map(|limit| {
                    let percent = u64::from(params.usage.input_tokens) * 100 / u64::from(limit);
                    u8::try_from(percent).unwrap_or(100).min(100)
                });
            }
            AcpEvent::ContextCleared(_) => {
                self.transcript.clear();
                self.tool_calls.clear();
                self.prompt_in_flight = false;
                self.context_percent = None;
                self.last_drained_kind = None;
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.transcript.push_user_message("[wisp-next] Context cleared");
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                if let Some(mut modal) = self.modal.take() {
                    modal.cancel();
                }
                if let Some(meta) = plan_review_meta(&params) {
                    self.screen_router.open_plan_review(meta, responder);
                    return;
                }
                self.modal = Some(ElicitationModal::new(params, responder));
            }
            AcpEvent::McpNotification(notification) => {
                if self
                    .modal
                    .as_mut()
                    .is_some_and(|modal| matches!(modal.on_notification(&notification), ModalOutcome::Close))
                {
                    self.modal = None;
                }
            }
            AcpEvent::ConnectionClosed => {
                if let Some(mut modal) = self.modal.take() {
                    modal.cancel();
                }
                self.screen_router.close();
                self.exit_requested = true;
            }
            _ => tracing::debug!("ignoring ACP event unsupported by the experimental UI"),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        if self.keybindings.exit.matches(key) {
            if self.ctrl_c_armed_at.is_some() {
                self.exit_requested = true;
            } else {
                self.composer.clear();
                self.ctrl_c_armed_at = Some(Instant::now());
            }
            return;
        }

        if self.screen_router.is_active() {
            if let Some(effect) = self.screen_router.on_key(key) {
                self.pending_screen_effects.push_back(effect);
            }
            return;
        }

        if let Some(modal) = self.modal.as_mut() {
            if matches!(modal.on_key(key), ModalOutcome::Close) {
                self.modal = None;
            }
            return;
        }

        if self.keybindings.toggle_git_diff.matches(key) {
            let effect = self.screen_router.open_git_diff(&self.working_dir);
            self.pending_screen_effects.push_back(effect);
            return;
        }

        if key.code == KeyCode::Enter && key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
            || key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.composer.insert_newline();
            return;
        }

        if self.composer.has_overlay() {
            self.on_overlay_key(key);
            return;
        }

        if self.keybindings.submit.matches(key) {
            self.submit();
            return;
        }

        if self.keybindings.cancel.matches(key) {
            if self.prompt_in_flight {
                let _ = self.prompt_handle.cancel(&self.session_id);
            }
            return;
        }

        match key.code {
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
                if self.keybindings.open_command_picker.matches(key) && self.composer.text() == "/" {
                    self.composer.open_command_picker(self.available_commands.clone());
                } else if self.keybindings.open_file_picker.matches(key) {
                    self.composer.open_file_picker(&self.working_dir);
                }
            }
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.move_left(),
            KeyCode::Right => self.composer.move_right(),
            KeyCode::Up if !self.composer.move_up() => {
                self.composer.recall_previous();
            }
            KeyCode::Down if !self.composer.move_down() => {
                self.composer.recall_next();
            }
            KeyCode::Home => self.composer.move_line_start(),
            KeyCode::End => self.composer.move_line_end(),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        self.composer.insert_str(text);
        self.composer.refresh_overlay_query();
    }

    pub fn on_tick(&mut self, now: Instant) {
        if let Some(armed_at) = self.ctrl_c_armed_at
            && now.duration_since(armed_at) > CTRL_C_CONFIRM_WINDOW
        {
            self.ctrl_c_armed_at = None;
        }
        if self.prompt_in_flight || self.tool_calls.any_running() {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
    }

    pub fn wants_tick(&self) -> bool {
        self.prompt_in_flight || self.tool_calls.any_running() || self.ctrl_c_armed_at.is_some()
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    pub fn full_screen_active(&self) -> bool {
        self.screen_router.is_active()
    }

    pub fn render_modal(
        &mut self,
        frame: &mut ratatui::Frame,
        theme: &crate::theme::Theme,
        highlighter: &mut crate::syntax::SyntaxHighlighter,
    ) {
        if self.screen_router.is_active() {
            self.screen_router.render(frame, theme, highlighter);
        } else if let Some(modal) = &self.modal {
            modal.render(frame, theme);
        }
    }

    pub fn on_screen_event(&mut self, event: ScreenEvent) {
        if let Some(effect) = self.screen_router.on_event(event) {
            self.pending_screen_effects.push_back(effect);
        }
    }

    pub fn take_screen_effect(&mut self) -> Option<ScreenEffect> {
        self.pending_screen_effects.pop_front()
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    /// Remove transcript segments that can never mutate again and resolve them
    /// for one-time handoff to the terminal presenter.
    pub fn drain_finalized(&mut self) -> Vec<HistoryItem> {
        let drained = self.transcript.drain_finalized_prefix(&self.tool_calls, self.prompt_in_flight);
        let items: Vec<HistoryItem> = drained
            .into_iter()
            .map(|segment| match segment {
                SegmentContent::ToolCall(id) => {
                    let entry = self.tool_calls.remove(&id).unwrap_or(ToolCallEntry {
                        title: id.clone(),
                        status: ToolStatus::Success,
                        diff: None,
                        id,
                    });
                    HistoryItem::Tool { title: entry.title, status: entry.status, diff: entry.diff }
                }
                other => resolve_plain_segment(other),
            })
            .collect();

        if let Some(last) = items.last() {
            self.last_drained_kind = Some(last.kind());
        }
        items
    }

    /// Segments still owned by the live viewport (streaming tail, running tools).
    pub fn pending_items(&self) -> Vec<HistoryItem> {
        self.transcript
            .pending()
            .iter()
            .map(|segment| match segment {
                SegmentContent::ToolCall(id) => {
                    let entry = self.tool_calls.entry(id);
                    HistoryItem::Tool {
                        title: entry.map_or_else(|| id.clone(), |e| e.title.clone()),
                        status: entry.map_or(ToolStatus::Success, |e| e.status.clone()),
                        diff: entry.and_then(|value| value.diff.clone()),
                    }
                }
                other => resolve_plain_segment(other.clone()),
            })
            .collect()
    }

    pub fn last_drained_kind(&self) -> Option<HistoryKind> {
        self.last_drained_kind
    }

    pub fn transcript_generation(&self) -> u64 {
        self.transcript_generation
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    pub fn content_padding(&self) -> usize {
        self.content_padding
    }

    pub fn workspace_label(&self) -> String {
        self.workspace_status.label()
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn context_percent(&self) -> Option<u8> {
        self.context_percent
    }

    pub fn busy(&self) -> bool {
        self.prompt_in_flight
    }

    pub fn exit_confirmation_active(&self) -> bool {
        self.ctrl_c_armed_at.is_some()
    }

    pub fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.composer.close_overlay(),
            KeyCode::Up => self.composer.overlay_move_up(),
            KeyCode::Down => self.composer.overlay_move_down(),
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(command) = self.composer.accept_command() {
                    if command.has_input {
                        self.composer.insert_char(' ');
                    } else {
                        self.submit();
                    }
                } else {
                    self.composer.accept_file();
                }
            }
            KeyCode::Backspace if self.composer.active_token_is_empty() => {
                self.composer.backspace();
                self.composer.close_overlay();
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                self.composer.refresh_overlay_query();
            }
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
                if character.is_whitespace() {
                    self.composer.close_overlay();
                } else {
                    self.composer.refresh_overlay_query();
                }
            }
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
                self.available_commands = update
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
                    })
                    .collect();
            }
            _ => {
                self.transcript.close_thought_block();
            }
        }
    }

    fn submit(&mut self) {
        if self.composer.is_empty() || self.prompt_in_flight {
            return;
        }

        let mentions = self.composer.selected_mentions();
        let text = self.composer.take_submission();
        let attachments = crate::attachments::build(&mentions);
        self.transcript.push_user_message(&text);
        for placeholder in &attachments.placeholders {
            self.transcript.push_user_message(placeholder);
        }
        for warning in &attachments.warnings {
            self.transcript.push_user_message(&format!("[wisp-next] {warning}"));
        }
        self.prompt_in_flight = true;
        let content = (!attachments.blocks.is_empty()).then_some(attachments.blocks);
        if let Err(e) = self.prompt_handle.prompt(&self.session_id, &text, content) {
            tracing::error!("failed to send prompt: {e}");
        }
    }

    fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
        self.prompt_in_flight = false;
        self.tool_calls.finalize_running(terminal_status);
        self.transcript.close_thought_block();
    }
}

fn plan_review_meta(
    params: &acp_utils::notifications::ElicitationParams,
) -> Option<utils::plan_review::PlanReviewElicitationMeta> {
    match &params.request {
        acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams { meta, .. } => {
            utils::plan_review::PlanReviewElicitationMeta::parse(meta.as_ref().map(|meta| &meta.0))
        }
        acp_utils::notifications::CreateElicitationRequestParams::UrlElicitationParams { .. } => None,
    }
}

fn resolve_plain_segment(segment: SegmentContent) -> HistoryItem {
    match segment {
        SegmentContent::UserMessage(text) => HistoryItem::User(text),
        SegmentContent::Text(text) => HistoryItem::Text(text),
        SegmentContent::Thought(text) => HistoryItem::Thought(text),
        SegmentContent::ToolCall(id) => HistoryItem::Tool { title: id, status: ToolStatus::Success, diff: None },
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
