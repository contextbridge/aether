use crate::attachments::PromptAttachment;
use crate::composer::Composer;
use crate::diff::DiffPreview;
use crate::dropped_files::parse_dropped_file_paths;
use crate::keybindings::Keybindings;
use crate::modal::{ElicitationModal, ModalOutcome};
use crate::picker::CommandEntry;
use crate::plan_tracker::PlanTracker;
use crate::progress_indicator::{ProgressActivity, ProgressIndicator, WorkspaceProgress};
use crate::prompt_search::PromptSearchMessage;
use crate::screen_router::{ScreenEffect, ScreenEvent, ScreenRouter};
use crate::screens::git_diff::GitDiffEvent;
use crate::session_loading_buffer::SessionLoadingBuffer;
use crate::session_picker::{SessionPicker, SessionPickerMessage};
use crate::settings::{ContextUsageDisplay, UiSettings, resolve_content_padding};
use crate::settings_overlay::{
    SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue, SettingsOverlay, SettingsOverlayMessage,
};
use crate::terminal_effects::{TerminalEffect, TerminalEffects};
use crate::tool_calls::{ToolCallEntry, ToolCallLog, ToolStatus};
use crate::transcript::{SegmentContent, Transcript};
use crate::workspace_picker::{WorkspacePicker, WorkspacePickerMessage};
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle};
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use acp_utils::notifications::AetherCapabilities;
use acp_utils::notifications::McpNotification;
use acp_utils::notifications::McpServerStatusEntry;
use agent_client_protocol::schema::{self as acp, SessionConfigOptionCategory, SessionId};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use utils::ReasoningEffort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSurface {
    Screen,
    Settings,
    SessionPicker,
    WorkspacePicker,
    Modal,
    PromptSearch,
    Overlay,
    Composer,
}

/// Root UI state: reduces terminal input and ACP events into the transcript,
/// tool-call log, and composer that the renderer draws each frame.
pub struct App {
    session_id: SessionId,
    agent_name: String,
    prompt_capabilities: acp::PromptCapabilities,
    session_capabilities: acp::SessionCapabilities,
    capabilities: AetherCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    prompt_handle: AcpPromptHandle,
    keybindings: Keybindings,
    workspace_status: WorkspaceStatus,
    content_padding: usize,
    working_dir: PathBuf,
    available_commands: Vec<CommandEntry>,
    session_loading_buffer: SessionLoadingBuffer,
    session_picker: Option<SessionPicker>,
    workspace_picker: Option<WorkspacePicker>,
    workspace_move_state: WorkspaceMoveState,
    transcript: Transcript,
    tool_calls: ToolCallLog,
    composer: Composer,
    prompt_in_flight: bool,
    context_usage: Option<ContextUsageDisplay>,
    unhealthy_server_count: usize,
    server_statuses: Vec<McpServerStatusEntry>,
    ctrl_c_armed_at: Option<Instant>,
    exit_requested: bool,
    spinner_tick: usize,
    compaction_active: bool,
    progress_indicator: ProgressIndicator,
    last_drained_kind: Option<HistoryKind>,
    transcript_generation: u64,
    modal: Option<ElicitationModal>,
    screen_router: ScreenRouter,
    pending_screen_effects: std::collections::VecDeque<ScreenEffect>,
    settings_overlay: Option<SettingsOverlay>,
    ui_settings: UiSettings,
    pending_theme: Option<crate::theme::Theme>,
    plan_tracker: PlanTracker,
    terminal_effects: TerminalEffects,
    last_terminal_size: (u16, u16),
    surface_rect: Option<Rect>,
}

pub struct AppConfig {
    pub session_id: SessionId,
    pub agent_name: String,
    pub workspace_status: WorkspaceStatus,
    pub prompt_capabilities: acp::PromptCapabilities,
    pub session_capabilities: acp::SessionCapabilities,
    pub config_options: Vec<acp::SessionConfigOption>,
    pub auth_methods: Vec<acp::AuthMethod>,
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
    Tool {
        title: String,
        status: ToolStatus,
        diff: Option<DiffPreview>,
        raw_input: String,
        display_value: Option<String>,
        sub_agents: Vec<SubAgentHistoryItem>,
    },
}

/// Rendered sub-agent entry for the tree view beneath a parent tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentHistoryItem {
    pub agent_name: String,
    pub done: bool,
    pub tools: Vec<SubAgentToolHistoryItem>,
}

/// A single tool call within a sub-agent, rendered as a tree leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentToolHistoryItem {
    pub name: String,
    pub arguments: String,
    pub display_value: Option<String>,
    pub status: ToolStatus,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMoveState {
    Idle,
    Listing,
    Picking,
    Moving,
    LoadingSession,
}

impl WorkspaceMoveState {
    pub fn is_idle(&self) -> bool {
        matches!(self, WorkspaceMoveState::Idle)
    }
}

const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

fn builtin_commands(capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
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

fn merge_builtins(agent_commands: Vec<CommandEntry>, capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
    let mut all = builtin_commands(capabilities);
    all.extend(agent_commands);
    all
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let content_padding = resolve_content_padding(&config.settings);
        let capabilities = AetherCapabilities::from_meta(config.session_capabilities.meta.as_ref());
        let initial_commands = builtin_commands(&capabilities);
        Self {
            session_id: config.session_id,
            agent_name: config.agent_name,
            prompt_capabilities: config.prompt_capabilities,
            capabilities,
            session_capabilities: config.session_capabilities,
            config_options: config.config_options,
            auth_methods: config.auth_methods,
            prompt_handle: config.prompt_handle,
            keybindings: Keybindings::default(),
            workspace_status: config.workspace_status,
            content_padding,
            working_dir: config.working_dir,
            available_commands: initial_commands,
            session_loading_buffer: SessionLoadingBuffer::new(),
            session_picker: None,
            workspace_picker: None,
            workspace_move_state: WorkspaceMoveState::Idle,
            transcript: Transcript::new(),
            tool_calls: ToolCallLog::new(),
            composer: Composer::new(),
            prompt_in_flight: false,
            context_usage: None,
            unhealthy_server_count: 0,
            server_statuses: Vec::new(),
            ctrl_c_armed_at: None,
            exit_requested: false,
            spinner_tick: 0,
            compaction_active: false,
            progress_indicator: ProgressIndicator::default(),
            last_drained_kind: None,
            transcript_generation: 0,
            modal: None,
            screen_router: ScreenRouter::new(),
            pending_screen_effects: std::collections::VecDeque::new(),
            settings_overlay: None,
            ui_settings: config.settings,
            pending_theme: None,
            plan_tracker: PlanTracker::default(),
            terminal_effects: TerminalEffects::default(),
            last_terminal_size: (0, 0),
            surface_rect: None,
        }
    }

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
                self.compaction_active = false;
                self.context_usage = None;
                self.last_drained_kind = None;
                self.progress_indicator.reset();
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.transcript.push_user_message("[wisp-next] Context cleared");
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                if let Some(mut modal) = self.modal.take() {
                    modal.cancel();
                }
                self.session_picker = None;
                if let Some(meta) = plan_review_meta(&params) {
                    self.screen_router.open_plan_review(meta, responder);
                    return;
                }
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_elicitation_request(params, responder);
                    return;
                }
                self.modal = Some(ElicitationModal::new(params, responder));
            }
            AcpEvent::McpNotification(notification) => {
                if let McpNotification::ServerStatus { ref servers } = notification {
                    self.server_statuses.clone_from(servers);
                    self.unhealthy_server_count = servers
                        .iter()
                        .filter(|s| !matches!(s.status, acp_utils::notifications::McpServerStatus::Connected { .. }))
                        .count();
                    if let Some(overlay) = &mut self.settings_overlay {
                        overlay.update_server_statuses(servers.clone());
                    }
                }
                if self
                    .modal
                    .as_mut()
                    .is_some_and(|modal| matches!(modal.on_notification(&notification), ModalOutcome::Close))
                {
                    self.modal = None;
                }
                if let McpNotification::UrlElicitationComplete(ref params) = notification
                    && let Some(overlay) = &mut self.settings_overlay
                {
                    overlay.on_url_elicitation_complete(params);
                }
            }
            AcpEvent::AuthMethodsUpdated(params) => {
                self.auth_methods.clone_from(&params.auth_methods);
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.update_auth_methods(params.auth_methods);
                }
            }
            AcpEvent::AuthenticateComplete { method_id } => {
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_authenticate_complete(&method_id);
                }
            }
            AcpEvent::AuthenticateFailed { method_id, error } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_authenticate_failed(&method_id);
                }
            }
            AcpEvent::ConnectionClosed => {
                if let Some(mut modal) = self.modal.take() {
                    modal.cancel();
                }
                self.session_picker = None;
                self.workspace_picker = None;
                self.workspace_move_state = WorkspaceMoveState::Idle;
                if let Some(mut overlay) = self.settings_overlay.take() {
                    overlay.cancel_pending_elicitation();
                }
                self.session_loading_buffer.clear();
                self.screen_router.close();
                self.surface_rect = None;
                self.terminal_effects.clear();
                self.exit_requested = true;
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
                self.modal = None;
                self.session_picker = Some(picker);
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
                self.session_picker = None;
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            AcpEvent::NewSessionCreated { session_id, config_options } => {
                self.session_loading_buffer.clear();
                if let Some(mut overlay) = self.settings_overlay.take() {
                    overlay.cancel_pending_elicitation();
                }
                let previous_selections = extract_config_selections(&self.config_options);
                self.session_id = session_id;
                self.config_options = config_options;
                self.transcript.clear();
                self.tool_calls.clear();
                self.plan_tracker.clear();
                self.prompt_in_flight = false;
                self.compaction_active = false;
                self.context_usage = None;
                self.last_drained_kind = None;
                self.progress_indicator.reset();
                self.transcript_generation = self.transcript_generation.wrapping_add(1);
                self.transcript.push_user_message("[wisp-next] New session created");
                self.restore_config_selections(&previous_selections);
            }
            AcpEvent::SessionPreviewLoaded(preview) => {
                if let Some(picker) = &mut self.session_picker {
                    picker.on_preview_loaded(preview);
                }
            }
            AcpEvent::SessionPreviewFailed { session_id, error } => {
                if let Some(picker) = &mut self.session_picker {
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
                self.modal = None;
                self.workspace_picker = Some(picker);
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

    #[allow(clippy::too_many_lines)]
    pub fn on_key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        'event: {
            if self.keybindings.exit.matches(key) {
                if self.ctrl_c_armed_at.is_some() {
                    self.exit_requested = true;
                } else {
                    self.composer.clear();
                    self.ctrl_c_armed_at = Some(Instant::now());
                }
                break 'event;
            }

            match self.active_surface() {
                ActiveSurface::Screen => {
                    if let Some(effect) = self.screen_router.on_key(key) {
                        self.pending_screen_effects.push_back(effect);
                    }
                    break 'event;
                }
                ActiveSurface::Settings => {
                    let messages =
                        self.settings_overlay.as_mut().map(|overlay| overlay.on_key(key)).unwrap_or_default();
                    for message in messages {
                        self.handle_settings_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::SessionPicker => {
                    let messages =
                        self.session_picker.as_mut().and_then(|picker| picker.on_key(key)).unwrap_or_default();
                    for message in messages {
                        self.handle_session_picker_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::WorkspacePicker => {
                    let messages =
                        self.workspace_picker.as_mut().and_then(|picker| picker.on_key(key)).unwrap_or_default();
                    for message in messages {
                        self.handle_workspace_picker_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::Modal => {
                    if self.modal.as_mut().is_some_and(|modal| matches!(modal.on_key(key), ModalOutcome::Close)) {
                        self.modal = None;
                    }
                    break 'event;
                }
                ActiveSurface::PromptSearch => {
                    if let Some(msg) = self.composer.prompt_search_on_key(key) {
                        match msg {
                            PromptSearchMessage::QueryChanged(query) if !query.trim().is_empty() => {
                                self.send_prompt_search_query(query);
                            }
                            PromptSearchMessage::QueryChanged(_) => self.composer.restore_prompt_search_draft(),
                            _ => {}
                        }
                    }
                    break 'event;
                }
                ActiveSurface::Overlay | ActiveSurface::Composer => {}
            }

            if self.keybindings.open_prompt_search.matches(key)
                && self.capabilities.prompt_search
                && !self.composer.has_overlay()
            {
                self.composer.open_prompt_search();
                break 'event;
            }

            if self.keybindings.toggle_git_diff.matches(key) {
                let effect = self.screen_router.open_git_diff(&self.working_dir);
                self.pending_screen_effects.push_back(effect);
                break 'event;
            }

            if key.code == KeyCode::Enter && key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
                || key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.composer.insert_newline();
                break 'event;
            }

            if self.active_surface() == ActiveSurface::Overlay {
                self.on_overlay_key(key);
                break 'event;
            }

            if self.keybindings.cycle_reasoning.matches(key) {
                if let Some((id, val)) = cycle_reasoning_option(&self.config_options)
                    && self.prompt_handle.set_config_option(&self.session_id, &id, &val).is_ok()
                {
                    update_config_option_value(&mut self.config_options, &id, &val);
                }
                break 'event;
            }

            if self.keybindings.cycle_mode.matches(key) {
                if let Some((id, val)) = cycle_quick_option(&self.config_options)
                    && self.prompt_handle.set_config_option(&self.session_id, &id, &val).is_ok()
                {
                    update_config_option_value(&mut self.config_options, &id, &val);
                }
                break 'event;
            }

            if self.keybindings.submit.matches(key) {
                self.submit();
                break 'event;
            }

            if self.keybindings.cancel.matches(key) {
                if self.prompt_in_flight {
                    let _ = self.prompt_handle.cancel(&self.session_id);
                }
                break 'event;
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
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.composer.move_line_start(),
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self.composer.move_line_end(),
                _ => {}
            }
        }
        self.refresh_progress();
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.active_surface() == ActiveSurface::PromptSearch {
            if let Some(msg) = self.composer.prompt_search_on_paste(text) {
                match msg {
                    PromptSearchMessage::QueryChanged(query) if !query.trim().is_empty() => {
                        self.send_prompt_search_query(query);
                    }
                    PromptSearchMessage::QueryChanged(_) => {
                        self.composer.restore_prompt_search_draft();
                    }
                    _ => {}
                }
            }
            return;
        }
        let added = parse_dropped_file_paths(text).is_some_and(|paths| self.composer.add_dropped_media(paths));
        if !added {
            self.composer.insert_paste(text);
        }
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
        self.refresh_progress();
        self.progress_indicator.on_tick();
        self.plan_tracker.on_tick(now);
    }

    pub fn wants_tick(&self) -> bool {
        self.prompt_in_flight
            || self.tool_calls.any_running()
            || !self.workspace_move_state.is_idle()
            || self.compaction_active
            || self.progress_indicator.is_active()
            || self.ctrl_c_armed_at.is_some()
            || self.plan_tracker.has_completed_in_grace_period()
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
            || self.session_picker.is_some()
            || self.workspace_picker.is_some()
            || self.settings_overlay.is_some()
    }

    pub fn has_session_picker(&self) -> bool {
        self.session_picker.is_some()
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.workspace_move_state
    }

    pub fn full_screen_active(&self) -> bool {
        self.screen_router.is_active()
    }

    pub fn active_surface(&self) -> ActiveSurface {
        if self.screen_router.is_active() {
            ActiveSurface::Screen
        } else if self.settings_overlay.is_some() {
            ActiveSurface::Settings
        } else if self.session_picker.is_some() {
            ActiveSurface::SessionPicker
        } else if self.workspace_picker.is_some() {
            ActiveSurface::WorkspacePicker
        } else if self.modal.is_some() {
            ActiveSurface::Modal
        } else if self.composer.has_prompt_search() {
            ActiveSurface::PromptSearch
        } else if self.composer.has_overlay() {
            ActiveSurface::Overlay
        } else {
            ActiveSurface::Composer
        }
    }

    pub fn render_modal(
        &mut self,
        frame: &mut ratatui::Frame,
        theme: &crate::theme::Theme,
        highlighter: &mut crate::syntax::SyntaxHighlighter,
    ) {
        match self.active_surface() {
            ActiveSurface::PromptSearch | ActiveSurface::Overlay => {}
            ActiveSurface::Composer => self.surface_rect = None,
            ActiveSurface::Screen => {
                self.surface_rect = Some(frame.area());
                self.screen_router.render(frame, theme, highlighter);
            }
            ActiveSurface::Settings => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let Some(overlay) = &self.settings_overlay {
                    overlay.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::SessionPicker => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let Some(picker) = &self.session_picker {
                    picker.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::WorkspacePicker => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let Some(picker) = &self.workspace_picker {
                    picker.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::Modal => {
                self.surface_rect = Some(frame.area());
                if let Some(modal) = &self.modal {
                    modal.render(frame, theme);
                }
            }
        }
    }

    pub fn on_screen_event(&mut self, event: ScreenEvent) {
        if let ScreenEvent::GitDiff(GitDiffEvent::SubmitReview { request_id: _, prompt }) = &event {
            if self.prompt_in_flight {
                return;
            }
            let prompt = prompt.clone();
            self.prompt_in_flight = true;
            self.transcript.push_user_message(&format!("[wisp-next] Submitted review of working tree diff.\n{prompt}"));
            if let Err(e) = self.prompt_handle.prompt(&self.session_id, &prompt, None) {
                tracing::error!("failed to send review prompt: {e}");
                self.prompt_in_flight = false;
                self.transcript.push_user_message(&format!("[wisp-next] Failed to send review: {e}"));
            } else {
                self.screen_router.close();
            }
            self.screen_router.on_event(event);
            return;
        }
        if let Some(effect) = self.screen_router.on_event(event) {
            self.pending_screen_effects.push_back(effect);
        }
    }

    pub fn take_screen_effect(&mut self) -> Option<ScreenEffect> {
        self.pending_screen_effects.pop_front()
    }

    pub fn take_terminal_effect(&mut self) -> Option<TerminalEffect> {
        self.terminal_effects.pop()
    }

    pub fn push_terminal_effect(&mut self, effect: TerminalEffect) {
        self.terminal_effects.push(effect);
    }

    pub fn count_pending_terminal_effects(&self) -> usize {
        self.terminal_effects.queue_len()
    }

    pub fn needs_mouse_capture(&self) -> bool {
        self.has_mouse_capturing_surface()
    }

    fn has_mouse_capturing_surface(&self) -> bool {
        match self.active_surface() {
            ActiveSurface::Composer => false,
            ActiveSurface::Modal => self.modal.as_ref().is_some_and(ElicitationModal::needs_mouse_capture),
            ActiveSurface::Screen
            | ActiveSurface::Settings
            | ActiveSurface::SessionPicker
            | ActiveSurface::WorkspacePicker
            | ActiveSurface::PromptSearch
            | ActiveSurface::Overlay => true,
        }
    }

    pub fn terminal_size(&self) -> (u16, u16) {
        self.last_terminal_size
    }

    pub fn surface_rect(&self) -> Option<Rect> {
        self.surface_rect
    }

    pub fn set_surface_rect(&mut self, rect: Rect) {
        self.surface_rect = Some(rect);
    }

    pub fn clear_surface_rect(&mut self) {
        self.surface_rect = None;
    }

    pub fn on_terminal_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Resize(width, height) => self.on_resize(width, height),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => {}
        }
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
                    let sub_agents = build_sub_agent_history_items(&self.tool_calls, &id);
                    let entry = self.tool_calls.remove(&id).unwrap_or(ToolCallEntry {
                        title: id.clone(),
                        status: ToolStatus::Success,
                        diff: None,
                        id: id.clone(),
                        raw_input: String::new(),
                        display_value: None,
                    });
                    HistoryItem::Tool {
                        title: entry.title,
                        status: entry.status,
                        diff: entry.diff,
                        raw_input: entry.raw_input,
                        display_value: entry.display_value,
                        sub_agents,
                    }
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
                        raw_input: entry.map_or(String::new(), |e| e.raw_input.clone()),
                        display_value: entry.and_then(|e| e.display_value.clone()),
                        sub_agents: build_sub_agent_history_items(&self.tool_calls, id),
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

    pub fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn prompt_capabilities(&self) -> &acp::PromptCapabilities {
        &self.prompt_capabilities
    }

    pub fn session_capabilities(&self) -> &acp::SessionCapabilities {
        &self.session_capabilities
    }

    pub fn config_options(&self) -> &[acp::SessionConfigOption] {
        &self.config_options
    }

    pub fn auth_methods(&self) -> &[acp::AuthMethod] {
        &self.auth_methods
    }

    pub fn supports_prompt_search(&self) -> bool {
        self.capabilities.prompt_search
    }

    pub fn supports_session_preview(&self) -> bool {
        self.capabilities.session_preview
    }

    pub fn supports_workspace_move(&self) -> bool {
        self.capabilities.workspace_move
    }

    pub fn content_padding(&self) -> usize {
        self.content_padding
    }

    pub fn workspace_label(&self) -> String {
        self.workspace_status.label()
    }

    pub fn workspace_status(&self) -> &WorkspaceStatus {
        &self.workspace_status
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn context_usage(&self) -> Option<ContextUsageDisplay> {
        self.context_usage
    }

    pub fn context_percent(&self) -> Option<u8> {
        self.context_usage.map(|usage| {
            let percent = u64::from(usage.used_tokens) * 100 / u64::from(usage.limit_tokens).max(1);
            u8::try_from(percent).unwrap_or(100).min(100)
        })
    }

    pub fn unhealthy_server_count(&self) -> usize {
        self.unhealthy_server_count
    }

    pub fn waiting_for_response(&self) -> bool {
        self.prompt_in_flight
    }

    pub fn ui_settings(&self) -> &UiSettings {
        &self.ui_settings
    }

    pub fn busy(&self) -> bool {
        self.prompt_in_flight
    }

    pub fn is_agent_busy(&self) -> bool {
        self.prompt_in_flight || self.tool_calls.any_running()
    }

    pub fn compaction_active(&self) -> bool {
        self.compaction_active
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        &self.progress_indicator
    }

    pub fn exit_confirmation_active(&self) -> bool {
        self.ctrl_c_armed_at.is_some()
    }

    pub fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    pub fn plan_entries(&mut self) -> &[acp::PlanEntry] {
        self.plan_tracker.cached_visible_entries()
    }

    pub fn has_plan(&self) -> bool {
        self.plan_tracker.has_entries()
    }

    pub fn plan_tracker_mut(&mut self) -> &mut PlanTracker {
        &mut self.plan_tracker
    }

    fn refresh_progress(&mut self) {
        let workspace = match self.workspace_move_state {
            WorkspaceMoveState::Moving => WorkspaceProgress::Moving,
            WorkspaceMoveState::LoadingSession => WorkspaceProgress::LoadingSession,
            _ => WorkspaceProgress::None,
        };
        self.progress_indicator.update(ProgressActivity {
            agent_busy: self.is_agent_busy(),
            workspace,
            compaction_active: self.compaction_active,
        });
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.composer.close_overlay(),
            KeyCode::Up => self.composer.overlay_move_up(),
            KeyCode::Down => self.composer.overlay_move_down(),
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(command) = self.composer.accept_command() {
                    if command.builtin {
                        self.dispatch_builtin_command(&command);
                    } else if command.has_input {
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
                if let Some(overlay) = &mut self.settings_overlay {
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

    fn submit(&mut self) {
        if self.composer.is_empty() || self.prompt_in_flight {
            return;
        }

        let mentions = self.composer.selected_mentions();
        let (text, pending_media) = self.composer.take_submission();
        let mut all_attachments: Vec<PromptAttachment> =
            mentions.into_iter().map(|m| PromptAttachment { path: m.path, display_name: m.display_name }).collect();
        all_attachments.extend(pending_media);

        let outcome = crate::attachments::build_attachments(&all_attachments);
        self.transcript.push_user_message(&text);
        for placeholder in &outcome.placeholders {
            self.transcript.push_user_message(placeholder);
        }
        for warning in &outcome.warnings {
            self.transcript.push_user_message(&format!("[wisp-next] {warning}"));
        }

        if let Some(message) = self.media_support_error(&outcome.blocks) {
            self.transcript.push_user_message(&format!("[wisp-next] {message}"));
            return;
        }

        self.prompt_in_flight = true;
        let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
        if let Err(e) = self.prompt_handle.prompt(&self.session_id, &text, content) {
            tracing::error!("failed to send prompt: {e}");
            self.prompt_in_flight = false;
            self.transcript.push_user_message(&format!("[wisp-next] Failed to send prompt: {e}"));
        }
    }

    fn finish_prompt(&mut self, terminal_status: &ToolStatus) {
        let was_in_flight = self.prompt_in_flight;
        self.prompt_in_flight = false;
        self.compaction_active = false;
        self.tool_calls.finalize_running(terminal_status);
        self.transcript.close_thought_block();
        if was_in_flight && matches!(terminal_status, ToolStatus::Success) {
            self.terminal_effects.push(TerminalEffect::Bell);
        }
    }

    fn on_resize(&mut self, width: u16, height: u16) {
        self.last_terminal_size = (width, height);
        self.surface_rect = None;
    }

    fn on_mouse(&mut self, event: crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        let Some(rect) = self.surface_rect else {
            return;
        };
        let col = event.column;
        let row = event.row;
        if col < rect.x || col >= rect.right() || row < rect.y || row >= rect.bottom() {
            return;
        }
        let local_x = col.saturating_sub(rect.x);
        let local_y = row.saturating_sub(rect.y);
        match event.kind {
            MouseEventKind::ScrollUp => self.surface_scroll_up(local_y, local_x),
            MouseEventKind::ScrollDown => self.surface_scroll_down(local_y, local_x),
            MouseEventKind::Down(_) => self.surface_click(local_y, local_x),
            _ => {}
        }
    }

    fn surface_scroll_up(&mut self, local_y: u16, local_x: u16) {
        match self.active_surface() {
            ActiveSurface::Screen => self.screen_router.on_mouse_scroll_up(local_y, local_x),
            ActiveSurface::Settings => {
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_mouse_scroll_up(local_y);
                }
            }
            ActiveSurface::SessionPicker => {
                if let Some(picker) = &mut self.session_picker {
                    picker.scroll_up();
                }
            }
            ActiveSurface::WorkspacePicker => {
                if let Some(picker) = &mut self.workspace_picker {
                    picker.scroll_up();
                }
            }
            ActiveSurface::Modal => {
                if let Some(modal) = &mut self.modal {
                    modal.on_mouse_scroll_up(local_y);
                }
            }
            ActiveSurface::PromptSearch => self.composer.prompt_search_move_up(),
            ActiveSurface::Overlay => self.composer.overlay_move_up(),
            ActiveSurface::Composer => {}
        }
    }

    fn surface_scroll_down(&mut self, local_y: u16, local_x: u16) {
        match self.active_surface() {
            ActiveSurface::Screen => self.screen_router.on_mouse_scroll_down(local_y, local_x),
            ActiveSurface::Settings => {
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_mouse_scroll_down(local_y);
                }
            }
            ActiveSurface::SessionPicker => {
                if let Some(picker) = &mut self.session_picker {
                    picker.scroll_down();
                }
            }
            ActiveSurface::WorkspacePicker => {
                if let Some(picker) = &mut self.workspace_picker {
                    picker.scroll_down();
                }
            }
            ActiveSurface::Modal => {
                if let Some(modal) = &mut self.modal {
                    modal.on_mouse_scroll_down(local_y);
                }
            }
            ActiveSurface::PromptSearch => self.composer.prompt_search_move_down(),
            ActiveSurface::Overlay => self.composer.overlay_move_down(),
            ActiveSurface::Composer => {}
        }
    }

    fn surface_click(&mut self, local_y: u16, local_x: u16) {
        match self.active_surface() {
            ActiveSurface::Screen => self.screen_router.on_mouse_click(local_y, local_x),
            ActiveSurface::Settings => {
                let messages = self
                    .settings_overlay
                    .as_mut()
                    .map(|overlay| overlay.on_mouse_click(local_y, self.surface_rect.unwrap_or_default()))
                    .unwrap_or_default();
                for message in messages {
                    self.handle_settings_message(message);
                }
            }
            ActiveSurface::SessionPicker => {
                if let Some(picker) = &mut self.session_picker {
                    picker.select_row(local_y.saturating_sub(1) as usize);
                }
            }
            ActiveSurface::WorkspacePicker => {
                if let Some(picker) = &mut self.workspace_picker {
                    picker.select_row(local_y.saturating_sub(1) as usize);
                }
            }
            ActiveSurface::Modal => {
                if let Some(modal) = &mut self.modal {
                    modal.on_mouse_click(local_y);
                }
            }
            ActiveSurface::PromptSearch => self.composer.prompt_search_select_row(local_y as usize),
            ActiveSurface::Overlay => self.composer.overlay_select_row(local_y as usize),
            ActiveSurface::Composer => {}
        }
    }

    fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && !self.prompt_capabilities.image {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && !self.prompt_capabilities.audio {
            return Some("ACP agent does not support audio input.".to_string());
        }

        let model_option =
            self.config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::Model.as_str())?;
        let acp::SessionConfigKind::Select(select) = &model_option.kind else {
            return None;
        };

        let values: Vec<_> =
            select.current_value.0.split(',').map(str::trim).filter(|value| !value.is_empty()).collect();

        if values.is_empty() {
            return None;
        }

        let flat_options: Vec<&acp::SessionConfigSelectOption> = match &select.options {
            acp::SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
            acp::SessionConfigSelectOptions::Grouped(groups) => groups.iter().flat_map(|g| g.options.iter()).collect(),
            _ => return None,
        };

        let selected_meta: Vec<_> = values
            .iter()
            .filter_map(|value| {
                flat_options
                    .iter()
                    .find(|option| option.value.0.as_ref() == *value)
                    .map(|option| SelectOptionMeta::from_meta(option.meta.as_ref()))
            })
            .collect();

        if selected_meta.len() != values.len() {
            return Some("Current model selection is missing prompt capability metadata.".into());
        }

        if requires_image && selected_meta.iter().any(|meta| !meta.supports_image) {
            return Some("Current model selection does not support image input.".to_string());
        }
        if requires_audio && selected_meta.iter().any(|meta| !meta.supports_audio) {
            return Some("Current model selection does not support audio input.".to_string());
        }

        None
    }

    fn send_prompt_search_query(&mut self, query: String) {
        let Some(generation) = self.composer.prompt_search_generation() else {
            return;
        };
        let params = acp_utils::notifications::PromptSearchParams { query, limit: None, search_generation: generation };
        if let Err(e) = self.prompt_handle.search_prompts(params) {
            self.composer.prompt_search_on_failed(generation, format!("search failed: {e}"));
        }
    }

    fn dispatch_builtin_command(&mut self, cmd: &CommandEntry) {
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
                self.settings_overlay = Some(overlay);
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

    fn handle_session_picker_message(&mut self, msg: SessionPickerMessage) {
        match msg {
            SessionPickerMessage::Close => {
                self.session_picker = None;
            }
            SessionPickerMessage::LoadSession { session_id, cwd } => {
                self.session_loading_buffer.begin_load(session_id.clone());
                if let Err(e) = self.prompt_handle.load_session(&session_id, &cwd) {
                    self.session_loading_buffer.remove(&session_id);
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to load session: {e}"));
                } else {
                    self.session_picker = None;
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

    fn handle_workspace_picker_message(&mut self, msg: WorkspacePickerMessage) {
        match msg {
            WorkspacePickerMessage::Close => {
                self.workspace_picker = None;
                self.workspace_move_state = WorkspaceMoveState::Idle;
            }
            WorkspacePickerMessage::Move { target } => {
                self.workspace_picker = None;
                self.workspace_move_state = WorkspaceMoveState::Moving;
                if let Err(e) = self.prompt_handle.move_workspace(&self.session_id, target) {
                    self.transcript.push_user_message(&format!("[wisp-next] Failed to move workspace: {e}"));
                    self.workspace_move_state = WorkspaceMoveState::Idle;
                }
            }
        }
    }

    fn on_workspace_moved(&mut self, new_cwd: std::path::PathBuf) {
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

    fn handle_settings_message(&mut self, msg: SettingsOverlayMessage) {
        match msg {
            SettingsOverlayMessage::Close => {
                if let Some(mut overlay) = self.settings_overlay.take() {
                    overlay.cancel_pending_elicitation();
                }
            }
            SettingsOverlayMessage::SetConfigOption { config_id, value } => {
                if self.prompt_handle.set_config_option(&self.session_id, &config_id, &value).is_ok() {
                    if let Some(overlay) = &mut self.settings_overlay {
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
                if let Some(overlay) = &mut self.settings_overlay {
                    overlay.on_authenticate_started(&method_id);
                }
                let _ = self.prompt_handle.authenticate(&method_id);
            }
        }
    }

    fn restore_config_selections(&mut self, previous: &[(String, String)]) {
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

fn build_sub_agent_history_items(tool_calls: &ToolCallLog, tool_id: &str) -> Vec<SubAgentHistoryItem> {
    let Some(agents) = tool_calls.sub_agent_states(tool_id) else {
        return Vec::new();
    };
    agents
        .iter()
        .map(|agent| SubAgentHistoryItem {
            agent_name: agent.agent_name.clone(),
            done: agent.done,
            tools: agent
                .tool_order
                .iter()
                .filter_map(|tool_id| agent.tool_calls.get(tool_id))
                .map(|tc| SubAgentToolHistoryItem {
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                    display_value: tc.display_value.clone(),
                    status: tc.status.clone(),
                })
                .collect(),
        })
        .collect()
}

fn resolve_plain_segment(segment: SegmentContent) -> HistoryItem {
    match segment {
        SegmentContent::UserMessage(text) => HistoryItem::User(text),
        SegmentContent::Text(text) => HistoryItem::Text(text),
        SegmentContent::Thought(text) => HistoryItem::Thought(text),
        SegmentContent::ToolCall(id) => HistoryItem::Tool {
            title: id,
            status: ToolStatus::Success,
            diff: None,
            raw_input: String::new(),
            display_value: None,
            sub_agents: Vec::new(),
        },
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

fn is_cycleable_mode_option(option: &acp::SessionConfigOption) -> bool {
    matches!(option.kind, acp::SessionConfigKind::Select(_))
        && option.category == Some(SessionConfigOptionCategory::Mode)
}

fn cycle_quick_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let option = config_options.iter().find(|option| is_cycleable_mode_option(option))?;

    let acp::SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    let acp::SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return None;
    };

    if options.is_empty() {
        return None;
    }

    let current_index = options.iter().position(|entry| entry.value == select.current_value).unwrap_or(0);
    let next_index = (current_index + 1) % options.len();
    options.get(next_index).map(|next| (option.id.0.to_string(), next.value.0.to_string()))
}

fn extract_reasoning_levels(config_options: &[acp::SessionConfigOption]) -> Vec<ReasoningEffort> {
    let Some(option) = config_options.iter().find(|o| o.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())
    else {
        return Vec::new();
    };
    let acp::SessionConfigKind::Select(ref select) = option.kind else {
        return Vec::new();
    };
    let acp::SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return Vec::new();
    };
    options.iter().filter_map(|o| o.value.0.as_ref().parse().ok()).collect()
}

fn extract_reasoning_effort(config_options: &[acp::SessionConfigOption]) -> Option<ReasoningEffort> {
    let option =
        config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())?;
    let acp::SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };
    ReasoningEffort::parse(&select.current_value.0).unwrap_or(None)
}

fn cycle_reasoning_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let levels = extract_reasoning_levels(config_options);
    if levels.is_empty() {
        return None;
    }

    let current = extract_reasoning_effort(config_options);
    let next = ReasoningEffort::cycle_within(current, &levels);
    Some((ConfigOptionId::ReasoningEffort.as_str().to_string(), ReasoningEffort::config_str(next).to_string()))
}

fn update_config_option_value(options: &mut [acp::SessionConfigOption], config_id: &str, value: &str) {
    let Some(option) = options.iter_mut().find(|option| option.id.0.as_ref() == config_id) else {
        return;
    };
    let acp::SessionConfigKind::Select(select) = &mut option.kind else {
        return;
    };
    select.current_value = value.to_string().into();
}

fn extract_config_selections(config_options: &[acp::SessionConfigOption]) -> Vec<(String, String)> {
    config_options
        .iter()
        .filter_map(|option| {
            if let acp::SessionConfigKind::Select(ref select) = option.kind {
                Some((option.id.0.to_string(), select.current_value.0.to_string()))
            } else {
                None
            }
        })
        .collect()
}

impl App {
    pub fn take_pending_theme(&mut self) -> Option<crate::theme::Theme> {
        self.pending_theme.take()
    }

    fn apply_theme_change(&mut self, value: &str) {
        let file = if value.is_empty() { None } else { Some(value.to_string()) };
        self.ui_settings.theme.file = file;
        if let Err(e) = crate::settings::save_settings(&self.ui_settings) {
            tracing::warn!("Failed to save theme settings: {e}");
        }
        let theme =
            if value.is_empty() { crate::theme::Theme::default() } else { crate::settings::load_theme_file(value) };
        self.pending_theme = Some(theme);
        if let Some(overlay) = &mut self.settings_overlay {
            overlay.apply_change(&crate::settings_overlay::SettingsChange {
                config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
                new_value: value.to_string(),
            });
        }
    }
}

fn build_theme_entries(settings: &UiSettings) -> Vec<SettingsMenuEntry> {
    use acp_utils::config_meta::SelectOptionMeta;
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let files = crate::settings::list_theme_files();
    let mut values: Vec<SettingsMenuValue> = Vec::new();

    values.push(SettingsMenuValue {
        value: String::new(),
        name: "Default".to_string(),
        description: Some("Built-in Nord theme".to_string()),
        is_disabled: false,
        meta: SelectOptionMeta::default(),
    });

    for file in &files {
        let display = file.trim_end_matches(".tmTheme").to_string();
        values.push(SettingsMenuValue {
            value: file.clone(),
            name: display,
            description: None,
            is_disabled: false,
            meta: SelectOptionMeta::default(),
        });
    }

    let current_file = settings.theme.file.as_deref().unwrap_or("");
    let current_value_index =
        if current_file.is_empty() { 0 } else { values.iter().position(|v| v.value == current_file).unwrap_or(0) };

    vec![SettingsMenuEntry {
        config_id: THEME_CONFIG_ID.to_string(),
        title: "Theme".to_string(),
        values,
        current_value_index,
        current_raw_value: current_file.to_string(),
        entry_kind: SettingsMenuEntryKind::Theme,
        multi_select: false,
        display_name: None,
    }]
}
