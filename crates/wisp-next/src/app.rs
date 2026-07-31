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
use crate::tool_calls::{SubAgentState, ToolCallLog, ToolStatus};
use crate::transcript::{SegmentContent, Transcript};
use crate::workspace_picker::WorkspacePicker;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::AcpPromptHandle;
use acp_utils::notifications::{AetherCapabilities, McpServerStatusEntry};
use agent_client_protocol::schema::{self as acp, SessionId};
use std::borrow::Cow;
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

impl Layer {
    /// Whether this layer replaces the conversation rather than drawing above
    /// it, which also stops the transcript committing to terminal scrollback.
    pub fn is_fullscreen(&self) -> bool {
        matches!(self, Self::GitDiff(_) | Self::PlanReview(_))
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

/// Something only the process outside the UI can do: work to run off the event
/// loop, or a change to the terminal itself.
///
/// One ordered queue rather than a field per effect, so the event loop has a
/// single thing to drain and effects happen in the order they were asked for.
#[derive(Debug)]
pub enum RuntimeEffect {
    /// Run this off the UI thread. Its result comes back through
    /// [`App::on_task_result`].
    Spawn(Task),
    /// Hand the presenter a newly chosen theme.
    SetTheme(crate::theme::Theme),
    RingBell,
    /// Clear the terminal's own scrollback of conversation content that scrolled
    /// out of the inline viewport, once the conversation it belonged to is gone.
    PurgeScrollback,
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

    fn drain_finalized(&mut self, prompt_in_flight: bool) -> Vec<HistoryItem<'static>> {
        let drained = self.transcript.drain_finalized_prefix(&self.tool_calls, prompt_in_flight);
        let items: Vec<HistoryItem<'static>> =
            drained.iter().map(|segment| resolve_history_segment(segment, &self.tool_calls).into_owned()).collect();
        // Only once every segment is detached from the log can the entries they
        // were resolved against be dropped.
        for segment in &drained {
            if let SegmentContent::ToolCall(id) = segment {
                self.tool_calls.remove(id);
            }
        }
        if let Some(last) = items.last() {
            self.last_drained_kind = Some(last.kind());
        }
        items
    }

    fn pending_items(&self) -> Vec<HistoryItem<'_>> {
        self.transcript.pending().iter().map(|segment| resolve_history_segment(segment, &self.tool_calls)).collect()
    }
}

struct PendingSubmission {
    text: String,
}

/// The theme change running off the event loop, and the newer choice waiting for
/// it. Only one runs at a time so the save and the switch happen in the order the
/// user made them.
#[derive(Default)]
struct ThemeChange {
    in_flight: bool,
    queued: Option<String>,
}

/// Root UI state: reduces terminal input and ACP events into the transcript,
/// tool-call log, and composer that the renderer draws each frame.
pub struct App {
    agent: Agent,
    ui: UiConfig,
    workspace_status: WorkspaceStatus,
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
    /// What the event loop still owes the outside world.
    effects: VecDeque<RuntimeEffect>,
    pending_submission: Option<PendingSubmission>,
    plan_tracker: PlanTracker,
    theme_change: ThemeChange,
}

/// The agent connection, and everything it told us about itself when the
/// session opened. Only the session id and config options change afterwards, as
/// sessions are swapped and settings are edited.
struct Agent {
    session_id: SessionId,
    name: String,
    handle: AcpPromptHandle,
    working_dir: PathBuf,
    prompt_capabilities: acp::PromptCapabilities,
    session_capabilities: acp::SessionCapabilities,
    capabilities: AetherCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
}

/// How the UI is configured, as opposed to what it is currently showing.
struct UiConfig {
    settings: UiSettings,
    keybindings: Keybindings,
    content_padding: usize,
    status_line: ResolvedStatusLineSettings,
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
///
/// Borrowed rather than owned, because the live tail is resolved afresh on every
/// frame while the agent works: owning it meant deep-copying each running tool's
/// diff and sub-agent tree ten times a second. Only [`App::drain_finalized`]
/// needs owned items, because it drops the log entries it resolved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem<'a> {
    User(Cow<'a, str>),
    Text(Cow<'a, str>),
    Thought(Cow<'a, str>),
    Tool {
        title: Cow<'a, str>,
        status: Cow<'a, ToolStatus>,
        diff: Option<Cow<'a, DiffPreview>>,
        raw_input: Cow<'a, str>,
        display_value: Option<Cow<'a, str>>,
        /// The sub-agent tree drawn beneath this tool, taken from the log as-is
        /// rather than projected into a parallel set of types.
        sub_agents: Cow<'a, [SubAgentState]>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryKind {
    User,
    Text,
    Thought,
    Tool,
}

impl HistoryItem<'_> {
    pub fn kind(&self) -> HistoryKind {
        match self {
            HistoryItem::User(_) => HistoryKind::User,
            HistoryItem::Text(_) => HistoryKind::Text,
            HistoryItem::Thought(_) => HistoryKind::Thought,
            HistoryItem::Tool { .. } => HistoryKind::Tool,
        }
    }

    /// Detaches the item from the log it was resolved against, so it can outlive
    /// the entries [`App::drain_finalized`] is about to drop.
    pub fn into_owned(self) -> HistoryItem<'static> {
        match self {
            HistoryItem::User(text) => HistoryItem::User(Cow::Owned(text.into_owned())),
            HistoryItem::Text(text) => HistoryItem::Text(Cow::Owned(text.into_owned())),
            HistoryItem::Thought(text) => HistoryItem::Thought(Cow::Owned(text.into_owned())),
            HistoryItem::Tool { title, status, diff, raw_input, display_value, sub_agents } => HistoryItem::Tool {
                title: Cow::Owned(title.into_owned()),
                status: Cow::Owned(status.into_owned()),
                diff: diff.map(|diff| Cow::Owned(diff.into_owned())),
                raw_input: Cow::Owned(raw_input.into_owned()),
                display_value: display_value.map(|value| Cow::Owned(value.into_owned())),
                sub_agents: Cow::Owned(sub_agents.into_owned()),
            },
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
            agent: Agent {
                session_id: config.session_id,
                name: config.agent_name,
                handle: config.prompt_handle,
                working_dir: config.working_dir,
                prompt_capabilities: config.prompt_capabilities,
                session_capabilities: config.session_capabilities,
                capabilities,
                config_options: config.config_options,
                auth_methods: config.auth_methods,
            },
            ui: UiConfig {
                settings: config.settings,
                keybindings: Keybindings::default(),
                content_padding,
                status_line,
            },
            workspace_status: config.workspace_status,
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
            effects: VecDeque::new(),
            pending_submission: None,
            plan_tracker: PlanTracker::default(),
            theme_change: ThemeChange::default(),
        }
    }

    /// The next thing the event loop owes the outside world, in the order it
    /// was asked for.
    pub fn take_effect(&mut self) -> Option<RuntimeEffect> {
        self.effects.pop_front()
    }

    fn spawn(&mut self, task: impl Into<Task>) {
        self.effects.push_back(RuntimeEffect::Spawn(task.into()));
    }

    fn ring_bell(&mut self) {
        self.effects.push_back(RuntimeEffect::RingBell);
    }

    /// Silences a bell the terminal has not been given yet, for the turns that
    /// end in something other than a reply worth announcing.
    fn cancel_bell(&mut self) {
        self.effects.retain(|effect| !matches!(effect, RuntimeEffect::RingBell));
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

    /// Whether any layer is open.
    pub fn has_layer(&self) -> bool {
        self.layer.is_some()
    }

    /// Test seam: the one layer whose identity a test asserts on rather than
    /// reaching for what it draws.
    pub fn has_session_picker(&self) -> bool {
        matches!(self.layer, Some(Layer::Sessions(_)))
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
        self.layer.as_ref().is_some_and(|layer| !layer.is_fullscreen())
    }

    pub fn full_screen_active(&self) -> bool {
        self.layer.as_ref().is_some_and(Layer::is_fullscreen)
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.workspace_move_state
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_state == ExitState::Exiting
    }

    /// Remove transcript segments that can never mutate again and resolve them
    /// for one-time handoff to the terminal presenter.
    pub fn drain_finalized(&mut self) -> Vec<HistoryItem<'static>> {
        self.conversation.drain_finalized(self.turn.prompt_in_flight)
    }

    /// Segments still owned by the live viewport (streaming tail, running tools).
    pub fn pending_items(&self) -> Vec<HistoryItem<'_>> {
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
        &self.agent.prompt_capabilities
    }

    pub fn session_capabilities(&self) -> &acp::SessionCapabilities {
        &self.agent.session_capabilities
    }

    pub fn config_options(&self) -> &[acp::SessionConfigOption] {
        &self.agent.config_options
    }

    pub fn auth_methods(&self) -> &[acp::AuthMethod] {
        &self.agent.auth_methods
    }

    pub fn content_padding(&self) -> usize {
        self.ui.content_padding
    }

    /// Everything the status line reads, gathered for one frame.
    pub fn status_line_model(&self) -> StatusLineModel<'_> {
        StatusLineModel {
            settings: &self.ui.status_line,
            config_options: &self.agent.config_options,
            workspace: &self.workspace_status,
            agent_name: &self.agent.name,
            content_padding: self.ui.content_padding,
            context_usage: self.turn.context_usage,
            unhealthy_servers: self.unhealthy_server_count,
            waiting_for_response: self.turn.prompt_in_flight,
            exit_confirmation: self.exit_state.is_confirming(),
        }
    }

    pub fn ui_settings(&self) -> &UiSettings {
        &self.ui.settings
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

    /// Test seam: the status line reads this through
    /// [`App::status_line_model`] rather than calling it.
    pub fn exit_confirmation_active(&self) -> bool {
        self.exit_state.is_confirming()
    }

    pub fn spinner_tick(&self) -> usize {
        self.turn.spinner_tick
    }

    pub fn plan_entries(&self) -> Vec<acp::PlanEntry> {
        self.plan_tracker.current_entries()
    }

    /// Reaches past the renderer for the integration tests, which assert on the
    /// state a frame is drawn from rather than on the frame.
    pub fn has_plan(&self) -> bool {
        self.plan_tracker.has_entries()
    }

    /// Test seam: drives the plan tracker's grace period without waiting on it.
    pub fn plan_tracker_mut(&mut self) -> &mut PlanTracker {
        &mut self.plan_tracker
    }

    /// Drops all conversation state. Every conversation swap funnels through
    /// here so each also emits exactly one terminal scrollback purge.
    fn reset_conversation(&mut self) {
        self.effects.push_back(RuntimeEffect::PurgeScrollback);
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

/// Status shown for a tool call whose log entry has already been dropped, so a
/// resolved segment can borrow one rather than allocate a fallback per frame.
static DRAINED_TOOL_STATUS: ToolStatus = ToolStatus::Success;

fn resolve_history_segment<'a>(segment: &'a SegmentContent, tool_calls: &'a ToolCallLog) -> HistoryItem<'a> {
    match segment {
        SegmentContent::UserMessage(text) => HistoryItem::User(Cow::Borrowed(text)),
        SegmentContent::Text(text) => HistoryItem::Text(Cow::Borrowed(text)),
        SegmentContent::Thought(text) => HistoryItem::Thought(Cow::Borrowed(text)),
        SegmentContent::ToolCall(id) => {
            let entry = tool_calls.entry(id);
            HistoryItem::Tool {
                title: Cow::Borrowed(entry.map_or(id.as_str(), |value| value.title.as_str())),
                status: Cow::Borrowed(entry.map_or(&DRAINED_TOOL_STATUS, |value| &value.status)),
                diff: entry.and_then(|value| value.diff.as_ref()).map(Cow::Borrowed),
                raw_input: Cow::Borrowed(entry.map_or("", |value| value.raw_input.as_str())),
                display_value: entry.and_then(|value| value.display_value.as_deref()).map(Cow::Borrowed),
                sub_agents: Cow::Borrowed(tool_calls.sub_agent_states(id).unwrap_or(&[])),
            }
        }
    }
}
