use crate::composer::Composer;
use crate::diff::DiffPreview;
use crate::generation::Generation;
use crate::keybindings::Keybindings;
use crate::modal::ElicitationModal;
use crate::picker::CommandEntry;
use crate::plan_tracker::PlanTracker;
use crate::progress_indicator::{ProgressActivity, ProgressIndicator, WorkspaceProgress};
use crate::screens::git_diff::GitDiffScreen;
use crate::screens::plan_review::PlanReviewScreen;
use crate::session_loading_buffer::SessionLoadingBuffer;
use crate::session_picker::SessionPicker;
use crate::settings::{
    ContextUsageDisplay, ResolvedStatusLineSettings, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::settings_overlay::SettingsOverlay;
use crate::status_line::StatusLineModel;
use crate::surface::Surface;
use crate::tasks::Task;
use crate::tool_calls::{ToolCallLog, ToolStatus};
use crate::transcript::{SegmentContent, Transcript};
use crate::workspace_picker::WorkspacePicker;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::AcpPromptHandle;
use acp_utils::notifications::{AetherCapabilities, McpServerStatusEntry};
use agent_client_protocol::schema::{self as acp, SessionId};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

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

/// The layer above the conversation that owns input while it is open. At most
/// one is open, and the set is closed, so the few ACP updates that need a
/// concrete surface just match on it.
pub enum Layer {
    Settings(SettingsOverlay),
    Sessions(SessionPicker),
    Workspaces(WorkspacePicker),
    Elicitation(ElicitationModal),
    // Boxed: the two full-screen views are an order of magnitude larger than
    // the overlays, and `App` holds one `Layer` for the whole session.
    GitDiff(Box<GitDiffScreen>),
    PlanReview(Box<PlanReviewScreen>),
}

/// Which layer is open, for the questions that do not need the layer itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Settings,
    Sessions,
    Workspaces,
    Elicitation,
    GitDiff,
    PlanReview,
}

impl Layer {
    pub fn kind(&self) -> LayerKind {
        match self {
            Self::Settings(_) => LayerKind::Settings,
            Self::Sessions(_) => LayerKind::Sessions,
            Self::Workspaces(_) => LayerKind::Workspaces,
            Self::Elicitation(_) => LayerKind::Elicitation,
            Self::GitDiff(_) => LayerKind::GitDiff,
            Self::PlanReview(_) => LayerKind::PlanReview,
        }
    }

    fn surface(&mut self) -> &mut dyn Surface {
        match self {
            Self::Settings(surface) => surface,
            Self::Sessions(surface) => surface,
            Self::Workspaces(surface) => surface,
            Self::Elicitation(surface) => surface,
            Self::GitDiff(surface) => surface.as_mut(),
            Self::PlanReview(surface) => surface.as_mut(),
        }
    }
}

impl LayerKind {
    /// Whether this layer replaces the conversation rather than drawing above
    /// it, which also stops the transcript committing to terminal scrollback.
    pub fn is_fullscreen(self) -> bool {
        matches!(self, Self::GitDiff | Self::PlanReview)
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

struct Conversation {
    transcript: Transcript,
    tool_calls: ToolCallLog,
    last_drained_kind: Option<HistoryKind>,
    generation: Generation,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            transcript: Transcript::new(),
            tool_calls: ToolCallLog::new(),
            last_drained_kind: None,
            generation: Generation::default(),
        }
    }
}

impl Conversation {
    fn clear(&mut self) {
        self.transcript.clear();
        self.tool_calls.clear();
        self.last_drained_kind = None;
        self.generation.bump();
    }

    fn drain_finalized(&mut self, prompt_in_flight: bool) -> Vec<HistoryItem> {
        let drained = self.transcript.drain_finalized_prefix(&self.tool_calls, prompt_in_flight);
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

    fn pending_items(&self) -> Vec<HistoryItem> {
        self.transcript.pending().iter().map(|segment| resolve_history_segment(segment, &self.tool_calls)).collect()
    }
}

struct PendingSubmission {
    text: String,
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
    layer: Option<Layer>,
    workspace_move_state: WorkspaceMoveState,
    conversation: Conversation,
    composer: Composer,
    turn: TurnState,
    unhealthy_server_count: usize,
    server_statuses: Vec<McpServerStatusEntry>,
    exit_state: ExitState,
    progress_indicator: ProgressIndicator,
    pending_tasks: VecDeque<Task>,
    pending_submission: Option<PendingSubmission>,
    ui_settings: UiSettings,
    pending_theme: Option<crate::theme::Theme>,
    plan_tracker: PlanTracker,
    pending_bell: Option<()>,
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
            layer: None,
            workspace_move_state: WorkspaceMoveState::Idle,
            conversation: Conversation::default(),
            composer: Composer::new(),
            turn: TurnState::default(),
            unhealthy_server_count: 0,
            server_statuses: Vec::new(),
            exit_state: ExitState::Idle,
            progress_indicator: ProgressIndicator::default(),
            pending_tasks: VecDeque::new(),
            pending_submission: None,
            ui_settings: config.settings,
            pending_theme: None,
            plan_tracker: PlanTracker::default(),
            pending_bell: None,
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
            || self.conversation.tool_calls.any_running()
            || !self.workspace_move_state.is_idle()
            || self.turn.compaction_active
            || self.progress_indicator.is_active()
            || self.exit_state.is_confirming()
            || self.plan_tracker.has_completed_in_grace_period()
    }

    /// Which layer is open, if any.
    pub fn layer_kind(&self) -> Option<LayerKind> {
        self.layer.as_ref().map(Layer::kind)
    }

    /// Runs `apply` when the settings overlay is open. Agent pushes that only
    /// affect what it displays are dropped when it is not.
    fn with_settings(&mut self, apply: impl FnOnce(&mut SettingsOverlay)) {
        if let Some(Layer::Settings(overlay)) = self.layer.as_mut() {
            apply(overlay);
        }
    }

    fn with_session_picker(&mut self, apply: impl FnOnce(&mut SessionPicker)) {
        if let Some(Layer::Sessions(picker)) = self.layer.as_mut() {
            apply(picker);
        }
    }

    /// Whether an overlay is open. Full-screen views are excluded: they replace
    /// the conversation rather than sitting above it.
    pub fn has_modal(&self) -> bool {
        self.layer_kind().is_some_and(|kind| !kind.is_fullscreen())
    }

    pub fn full_screen_active(&self) -> bool {
        self.layer_kind().is_some_and(LayerKind::is_fullscreen)
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.workspace_move_state
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_state == ExitState::Exiting
    }

    /// Remove transcript segments that can never mutate again and resolve them
    /// for one-time handoff to the terminal presenter.
    pub fn drain_finalized(&mut self) -> Vec<HistoryItem> {
        self.conversation.drain_finalized(self.turn.prompt_in_flight)
    }

    /// Segments still owned by the live viewport (streaming tail, running tools).
    pub fn pending_items(&self) -> Vec<HistoryItem> {
        self.conversation.pending_items()
    }

    pub fn last_drained_kind(&self) -> Option<HistoryKind> {
        self.conversation.last_drained_kind
    }

    pub fn transcript_generation(&self) -> Generation {
        self.conversation.generation
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

    pub fn content_padding(&self) -> usize {
        self.content_padding
    }

    /// Everything the status line reads, gathered for one frame.
    pub fn status_line_model(&self) -> StatusLineModel<'_> {
        StatusLineModel {
            settings: &self.status_line,
            config_options: &self.config_options,
            workspace: &self.workspace_status,
            agent_name: &self.agent_name,
            content_padding: self.content_padding,
            context_usage: self.turn.context_usage,
            unhealthy_servers: self.unhealthy_server_count,
            waiting_for_response: self.turn.prompt_in_flight,
            exit_confirmation: self.exit_state.is_confirming(),
        }
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
        self.turn.prompt_in_flight || self.conversation.tool_calls.any_running()
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
        self.pending_submission = None;
        self.conversation.clear();
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
                .tool_calls
                .iter()
                .map(|call| SubAgentToolHistoryItem {
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    display_value: call.display_value.clone(),
                    status: call.status.clone(),
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
