use crate::attachments::PromptAttachment;
use crate::composer::Composer;
use crate::diff::DiffPreview;
use crate::dropped_files::parse_dropped_file_paths;
use crate::keybindings::Keybindings;
use crate::modal::ElicitationModal;
use crate::picker::CommandEntry;
use crate::plan_tracker::PlanTracker;
use crate::progress_indicator::{ProgressActivity, ProgressIndicator, WorkspaceProgress};
use crate::screens::{ScreenEffect, ScreenEvent};
use crate::selection::Direction;
use crate::session_config_view::SessionConfigView;
use crate::session_loading_buffer::SessionLoadingBuffer;
use crate::session_picker::SessionPicker;
use crate::settings::{
    ContextUsageDisplay, ResolvedStatusLineSettings, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::settings_overlay::{SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue, SettingsOverlay};
use crate::surface::{Surface, SurfaceMessage};
use crate::tool_calls::{ToolCallLog, ToolStatus};
use crate::transcript::{SegmentContent, Transcript};
use crate::workspace_picker::WorkspacePicker;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle};
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

#[path = "app/acp.rs"]
mod acp_reducer;
mod config;
mod input;
mod session;
mod submission;
use input::CTRL_C_CONFIRM_WINDOW;
use session::builtin_commands;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExitState {
    #[default]
    Idle,
    Confirming(Instant),
    Exiting,
}

impl ExitState {
    fn is_confirming(&self) -> bool {
        matches!(self, ExitState::Confirming(_))
    }
}

/// The layer above the conversation that owns input. At most one is open.
///
/// Input and rendering go through [`Layer::active`], so only the ACP updates
/// that need a concrete type match on the variants. Full-screen views are
/// type-erased because nothing is ever routed to one by type.
#[allow(clippy::large_enum_variant)]
#[derive(Default)]
enum Layer {
    #[default]
    None,
    Settings(SettingsOverlay),
    SessionPicker(SessionPicker),
    WorkspacePicker(WorkspacePicker),
    Elicitation(ElicitationModal),
    Screen(Box<dyn Surface>),
}

impl Layer {
    /// Whether an overlay is open. Full-screen views are excluded: they replace
    /// the conversation rather than sitting above it.
    fn is_overlay(&self) -> bool {
        !matches!(self, Layer::None | Layer::Screen(_))
    }

    fn active(&mut self) -> Option<&mut dyn Surface> {
        match self {
            Layer::None => None,
            Layer::Settings(overlay) => Some(overlay),
            Layer::SessionPicker(picker) => Some(picker),
            Layer::WorkspacePicker(picker) => Some(picker),
            Layer::Elicitation(modal) => Some(modal),
            Layer::Screen(screen) => Some(screen.as_mut()),
        }
    }

    fn active_ref(&self) -> Option<&dyn Surface> {
        match self {
            Layer::None => None,
            Layer::Settings(overlay) => Some(overlay),
            Layer::SessionPicker(picker) => Some(picker),
            Layer::WorkspacePicker(picker) => Some(picker),
            Layer::Elicitation(modal) => Some(modal),
            Layer::Screen(screen) => Some(screen.as_ref()),
        }
    }
}

/// Everything that belongs to the conversation turn in progress.
///
/// Grouping these means [`App::reset_turn_state`] cannot forget one and leave a
/// stale indicator on screen when the conversation is swapped out.
#[derive(Default)]
struct TurnState {
    prompt_in_flight: bool,
    compaction_active: bool,
    context_usage: Option<ContextUsageDisplay>,
    submitted_prompt_count: usize,
    spinner_tick: usize,
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
    status_line: ResolvedStatusLineSettings,
    working_dir: PathBuf,
    available_commands: Vec<CommandEntry>,
    session_loading_buffer: SessionLoadingBuffer,
    layer: Layer,
    workspace_move_state: WorkspaceMoveState,
    transcript: Transcript,
    tool_calls: ToolCallLog,
    composer: Composer,
    turn: TurnState,
    unhealthy_server_count: usize,
    server_statuses: Vec<McpServerStatusEntry>,
    exit_state: ExitState,
    progress_indicator: ProgressIndicator,
    last_drained_kind: Option<HistoryKind>,
    transcript_generation: u64,
    pending_screen_effects: std::collections::VecDeque<ScreenEffect>,
    ui_settings: UiSettings,
    pending_theme: Option<crate::theme::Theme>,
    plan_tracker: PlanTracker,
    pending_bell: Option<()>,
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

impl App {
    pub fn new(config: AppConfig) -> Self {
        let content_padding = resolve_content_padding(&config.settings);
        let status_line = resolve_status_line_settings(&config.settings);
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
            status_line,
            working_dir: config.working_dir,
            available_commands: initial_commands,
            session_loading_buffer: SessionLoadingBuffer::new(),
            layer: Layer::None,
            workspace_move_state: WorkspaceMoveState::Idle,
            transcript: Transcript::new(),
            tool_calls: ToolCallLog::new(),
            composer: Composer::new(),
            turn: TurnState::default(),
            unhealthy_server_count: 0,
            server_statuses: Vec::new(),
            exit_state: ExitState::Idle,
            progress_indicator: ProgressIndicator::default(),
            last_drained_kind: None,
            transcript_generation: 0,
            pending_screen_effects: std::collections::VecDeque::new(),
            ui_settings: config.settings,
            pending_theme: None,
            plan_tracker: PlanTracker::default(),
            pending_bell: None,
            last_terminal_size: (0, 0),
            surface_rect: None,
        }
    }

    pub fn on_tick(&mut self, now: Instant) {
        if let ExitState::Confirming(armed_at) = self.exit_state
            && now.duration_since(armed_at) > CTRL_C_CONFIRM_WINDOW
        {
            self.exit_state = ExitState::Idle;
        }
        self.refresh_progress();
        if self.progress_indicator.is_active() {
            self.turn.spinner_tick = self.turn.spinner_tick.wrapping_add(1);
        }
        self.plan_tracker.on_tick(now);
    }

    pub fn wants_tick(&self) -> bool {
        self.turn.prompt_in_flight
            || self.tool_calls.any_running()
            || !self.workspace_move_state.is_idle()
            || self.turn.compaction_active
            || self.progress_indicator.is_active()
            || self.exit_state.is_confirming()
            || self.plan_tracker.has_completed_in_grace_period()
    }

    pub fn has_modal(&self) -> bool {
        self.layer.is_overlay()
    }

    pub fn has_session_picker(&self) -> bool {
        matches!(self.layer, Layer::SessionPicker(_))
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.workspace_move_state
    }

    pub fn full_screen_active(&self) -> bool {
        self.layer.active_ref().is_some_and(Surface::is_fullscreen)
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_state == ExitState::Exiting
    }

    /// Remove transcript segments that can never mutate again and resolve them
    /// for one-time handoff to the terminal presenter.
    pub fn drain_finalized(&mut self) -> Vec<HistoryItem> {
        let drained = self.transcript.drain_finalized_prefix(&self.tool_calls, self.turn.prompt_in_flight);
        let mut items = Vec::with_capacity(drained.len());
        for segment in drained {
            let tool_id = match &segment {
                SegmentContent::ToolCall(id) => Some(id.clone()),
                _ => None,
            };
            items.push(resolve_history_segment(&segment, &self.tool_calls));
            if let Some(id) = tool_id {
                self.tool_calls.remove(&id);
            }
        }

        if let Some(last) = items.last() {
            self.last_drained_kind = Some(last.kind());
        }
        items
    }

    /// Segments still owned by the live viewport (streaming tail, running tools).
    pub fn pending_items(&self) -> Vec<HistoryItem> {
        self.transcript.pending().iter().map(|segment| resolve_history_segment(segment, &self.tool_calls)).collect()
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

    pub fn status_line_settings(&self) -> &ResolvedStatusLineSettings {
        &self.status_line
    }

    pub fn workspace_status(&self) -> &WorkspaceStatus {
        &self.workspace_status
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn context_usage(&self) -> Option<ContextUsageDisplay> {
        self.turn.context_usage
    }

    pub fn context_percent(&self) -> Option<u8> {
        self.turn.context_usage.map(|usage| {
            let percent = u64::from(usage.used_tokens) * 100 / u64::from(usage.limit_tokens).max(1);
            u8::try_from(percent).unwrap_or(100).min(100)
        })
    }

    pub fn unhealthy_server_count(&self) -> usize {
        self.unhealthy_server_count
    }

    pub fn ui_settings(&self) -> &UiSettings {
        &self.ui_settings
    }

    /// A prompt is outstanding, so the agent owes us a reply.
    pub fn waiting_for_response(&self) -> bool {
        self.turn.prompt_in_flight
    }

    /// Either the prompt or one of its tool calls is still running.
    pub fn is_agent_busy(&self) -> bool {
        self.turn.prompt_in_flight || self.tool_calls.any_running()
    }

    pub fn compaction_active(&self) -> bool {
        self.turn.compaction_active
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        &self.progress_indicator
    }

    pub fn exit_confirmation_active(&self) -> bool {
        self.exit_state.is_confirming()
    }

    pub fn spinner_tick(&self) -> usize {
        self.turn.spinner_tick
    }

    pub fn plan_entries(&self) -> Vec<acp::PlanEntry> {
        self.plan_tracker.current_entries()
    }

    pub fn has_plan(&self) -> bool {
        self.plan_tracker.has_entries()
    }

    pub fn plan_tracker_mut(&mut self) -> &mut PlanTracker {
        &mut self.plan_tracker
    }

    /// Drops everything tied to the current conversation.
    ///
    /// Every path that swaps conversations — clearing context, creating a
    /// session, loading one, moving workspace — goes through this, so none of
    /// them can forget a field and leave stale state on screen.
    fn reset_conversation(&mut self) {
        self.reset_turn_state();
        self.transcript.clear();
        self.tool_calls.clear();
        self.last_drained_kind = None;
        self.transcript_generation = self.transcript_generation.wrapping_add(1);
    }

    /// Clears the per-turn indicators that must not survive into a different
    /// conversation. Used on its own when a load lands, because the transcript
    /// was already cleared when that load was requested — and may since have
    /// gained notices the user still needs to see.
    fn reset_turn_state(&mut self) {
        self.plan_tracker.clear();
        self.progress_indicator.reset();
        // The spinner phase is cosmetic and survives, so a swap does not make
        // the indicator visibly jump.
        let spinner_tick = self.turn.spinner_tick;
        self.turn = TurnState { spinner_tick, ..TurnState::default() };
    }

    fn refresh_progress(&mut self) {
        let workspace = match self.workspace_move_state {
            WorkspaceMoveState::Moving => WorkspaceProgress::Moving,
            WorkspaceMoveState::LoadingSession => WorkspaceProgress::LoadingSession,
            _ => WorkspaceProgress::None,
        };
        self.progress_indicator.update(
            ProgressActivity {
                agent_busy: self.is_agent_busy(),
                workspace,
                compaction_active: self.turn.compaction_active,
            },
            self.turn.submitted_prompt_count,
        );
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

fn resolve_history_segment(segment: &SegmentContent, tool_calls: &ToolCallLog) -> HistoryItem {
    match segment {
        SegmentContent::UserMessage(text) => HistoryItem::User(text.clone()),
        SegmentContent::Text(text) => HistoryItem::Text(text.clone()),
        SegmentContent::Thought(text) => HistoryItem::Thought(text.clone()),
        SegmentContent::ToolCall(id) => {
            let entry = tool_calls.entry(id);
            HistoryItem::Tool {
                title: entry.map_or_else(|| id.clone(), |value| value.title.clone()),
                status: entry.map_or(ToolStatus::Success, |value| value.status.clone()),
                diff: entry.and_then(|value| value.diff.clone()),
                raw_input: entry.map_or_else(String::new, |value| value.raw_input.clone()),
                display_value: entry.and_then(|value| value.display_value.clone()),
                sub_agents: build_sub_agent_history_items(tool_calls, id),
            }
        }
    }
}
