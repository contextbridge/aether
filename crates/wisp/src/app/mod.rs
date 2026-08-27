use crate::command::{AgentCommand, Command, CommandResult, FailedCommand};
use crate::app::keybindings::Keybindings;
use crate::app::message::Message;
use crate::conversation::items::{Conversation, ConversationItem};
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressPhase};
use crate::conversation::status_line::StatusLineModel;
use crate::conversation::tool_calls::ToolStatus;
use crate::session::platform::{
    BrowserOpener, ClipboardWriter, default_browser_opener, default_clipboard_writer,
};
use crate::session::session_config_view::LocalConfigOption;
use crate::session::session_model::SessionModel;
use crate::session::workspace_status::WorkspaceStatus;
use crate::settings::{
    ResolvedStatusLineSettings, SettingsModel, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::surfaces::composer::Composer;
use crate::surfaces::input::RootOutput;
use crate::surfaces::workspace_picker::WorkspacePicker;
use crate::surfaces::picker::CommandEntry;
use crate::view::generation::Generation;
use crate::theme::Theme;
use acp_utils::client::AcpEvent;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::v1::{self as acp, SessionId};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc;

pub mod message;
mod navigation;

pub use crate::session::session_model::WorkspaceMoveState;
pub use navigation::{Overlay, Route};

mod acp_reducer;
mod config;
mod input;
mod keybindings;
mod session;
mod submission;
use config::build_theme_entries;
use input::CTRL_C_CONFIRM_WINDOW;
use session::builtin_commands;
use submission::SubmissionState;

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

/// Root UI state: reduces terminal input and ACP events into the canonical
/// conversation, feature state, and composer that the renderer draws each frame.
pub struct App {
    session: SessionModel,
    ui: UiConfig,
    available_commands: Vec<CommandEntry>,
    route: Route,
    overlay: Option<Overlay>,
    conversation: Conversation,
    composer: Composer,
    exit_state: ExitState,
    /// What the event loop still owes the outside world.
    commands: VecDeque<Command>,
    submission: SubmissionState,
    browser_opener: BrowserOpener,
    clipboard_writer: ClipboardWriter,
}

/// How the UI is configured, as opposed to what it is currently showing.
struct UiConfig {
    settings: SettingsModel,
    keybindings: Keybindings,
    content_padding: usize,
    status_line: ResolvedStatusLineSettings,
    theme: Theme,
    theme_generation: Generation,
}

pub struct AppConfig {
    pub session_id: SessionId,
    pub agent_name: String,
    pub workspace_status: WorkspaceStatus,
    pub prompt_capabilities: acp::PromptCapabilities,
    pub session_capabilities: acp::SessionCapabilities,
    pub config_options: Vec<acp::SessionConfigOption>,
    pub auth_methods: Vec<acp::AuthMethod>,
    pub working_dir: PathBuf,
    pub settings: UiSettings,
    /// Host services the UI reaches for; injected so tests observe URL opens
    /// and clipboard writes without spawning anything.
    pub browser_opener: BrowserOpener,
    pub clipboard_writer: ClipboardWriter,
}

impl App {
    /// Build the UI from a freshly connected ACP session.
    ///
    /// Returns the pieces only the process outside the UI needs: the event
    /// channel feeding the event loop and the handle commands are sent through.
    /// Crate-internal entry-point wiring, not part of the public or test API.
    pub(crate) fn from_session(
        session: crate::session::Session,
        settings: UiSettings,
    ) -> (Self, mpsc::UnboundedReceiver<AcpEvent>, acp_utils::client::AcpClientHandle) {
        let crate::session::Session {
            session_id,
            agent_name,
            prompt_capabilities,
            session_capabilities,
            config_options,
            auth_methods,
            event_rx,
            client_handle,
            working_dir,
            workspace_status,
        } = session;
        let mut app = Self::new(AppConfig {
            session_id,
            agent_name,
            workspace_status,
            prompt_capabilities,
            session_capabilities,
            config_options,
            auth_methods,
            working_dir,
            settings,
            browser_opener: default_browser_opener(),
            clipboard_writer: default_clipboard_writer(),
        });
        app.queue(Command::ResolveWorkspace { cwd: app.session.working_dir().to_path_buf() });
        (app, event_rx, client_handle)
    }

    pub fn new(config: AppConfig) -> Self {
        let ui = UiConfig {
            content_padding: resolve_content_padding(&config.settings),
            status_line: resolve_status_line_settings(&config.settings),
            keybindings: Keybindings::from_settings(&config.settings),
            theme: Theme::load(&config.settings),
            theme_generation: Generation::default(),
            settings: SettingsModel::new(config.settings.clone()),
        };
        let capabilities = AetherCapabilities::from_meta(config.session_capabilities.meta.as_ref());
        let initial_commands = builtin_commands(&capabilities);
        let browser_opener = config.browser_opener.clone();
        let clipboard_writer = config.clipboard_writer.clone();
        Self {
            session: SessionModel::from_config(config, capabilities),
            ui,
            available_commands: initial_commands,
            route: Route::Conversation,
            overlay: None,
            conversation: Conversation::default(),
            composer: Composer::new(),
            exit_state: ExitState::Idle,
            commands: VecDeque::new(),
            submission: SubmissionState::default(),
            browser_opener,
            clipboard_writer,
        }
    }

    /// Reduce one external input and return its commands.
    ///
    /// This is the synchronous model boundary used by the runtime dispatcher.
    pub fn update(&mut self, message: Message) -> Vec<Command> {
        match message {
            Message::Terminal(event) => self.on_terminal_event(event),
            Message::Agent(event) => self.on_acp_event(*event),
            Message::CommandFinished(result) => self.on_command_result(*result),
            Message::Tick(now) => self.on_tick(now),
        }

        self.refresh_progress();
        self.take_commands()
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.commands.drain(..).collect()
    }

    pub fn on_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::AgentCommandAccepted => {}
            CommandResult::ConfigOptionsUpdated(options) => {
                self.session.update_config_options(options);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_config_options(self.session.config_options());
                }
            }
            CommandResult::ConfigOptionUpdateFailed { error } => {
                tracing::warn!("set_session_config_option failed: {error}");
                self.notify(&format!("Failed to update setting: {error}"));
            }
            CommandResult::AuthenticationCompleted { method_id } => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_complete(&method_id);
                }
            }
            CommandResult::AuthenticationFailed { method_id } => {
                tracing::warn!("Provider authentication failed for {method_id}");
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_failed(&method_id);
                }
            }
            CommandResult::FilesIndexed { request_id, files } => self.composer.on_files_indexed(request_id, files),
            CommandResult::GitDiff(event) => {
                let Route::GitReview(screen) = &mut self.route else { return };
                let outputs = screen.on_event(event).into_iter().map(RootOutput::GitReview).collect();
                self.dispatch_outputs(outputs);
            }
            CommandResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            CommandResult::ThemesListed(files) => {
                let entries = build_theme_entries(self.ui.settings.ui(), &files);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.upsert_local_entries(entries);
                }
            }
            CommandResult::ThemeApplied { settings, theme, error } => self.finish_theme_change(settings, theme, error),
            CommandResult::WorkspaceResolved { cwd, status } => {
                if self.session.working_dir() == cwd {
                    self.session.set_workspace_status(status);
                }
            }
            CommandResult::SessionsListed(response) => self.open_session_picker(response.sessions),
            CommandResult::SessionLoaded(loaded) => self.on_loaded_session(loaded),
            CommandResult::NewSessionCreated(response) => {
                self.on_new_session(response.session_id, response.config_options.unwrap_or_default());
            }
            CommandResult::PromptSearchResults(response) => self.composer.prompt_search_on_results(response),
            CommandResult::PromptSearchFailed { query, error } => {
                if let Some(picker) = self.composer.prompt_search_mut() {
                    picker.on_failed(&query, error);
                }
            }
            CommandResult::SessionPreviewLoaded(preview) => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_loaded(preview);
                }
            }
            CommandResult::SessionPreviewFailed { session_id, error } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_failed(&session_id, error);
                }
            }
            CommandResult::WorkspacesListed(response) => {
                self.open_overlay(Overlay::Workspaces(WorkspacePicker::new(response.workspaces)));
                self.session.begin_workspace_picking();
            }
            CommandResult::WorkspaceListFailed { error } => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            CommandResult::WorkspaceMoved(response) => self.on_workspace_moved(response.new_cwd),
            CommandResult::WorkspaceMoveFailed { error } => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            CommandResult::Failed { command, error } => self.on_command_failed(command, &error),
        }
    }

    fn on_command_failed(&mut self, command: FailedCommand, error: &str) {
        match command {
            FailedCommand::Prompt => {
                self.finish_prompt(&ToolStatus::Error(format!("failed: {error}")));
                self.submission.reset();
            }
            FailedCommand::LoadSession | FailedCommand::ListWorkspaces | FailedCommand::MoveWorkspace => {
                self.session.end_workspace_move();
            }
            FailedCommand::Other(_) => {}
        }
        self.notify(&format!("Failed to {}: {error}", command.describe()));
    }

    fn start_prompt(&mut self, text: String, content: Option<Vec<acp::ContentBlock>>) {
        self.conversation.turn_mut().set_prompt_in_flight(true);
        self.conversation.progress_indicator_mut().prompt_started();
        self.queue(Command::Agent(AgentCommand::Prompt {
            session_id: self.session.session_id().clone(),
            text,
            content,
        }));
    }

    fn queue(&mut self, command: Command) {
        self.commands.push_back(command);
    }

    pub fn on_tick(&mut self, now: Instant) {
        if let ExitState::Confirming(armed_at) = self.exit_state
            && now.duration_since(armed_at) > CTRL_C_CONFIRM_WINDOW
        {
            self.exit_state = ExitState::Idle;
        }
        if self.conversation.progress_indicator().is_active() {
            self.conversation.turn_mut().advance_spinner();
        }
        self.conversation.progress_indicator_mut().on_tick(now);
        self.conversation.plan_tracker_mut().on_tick(now);
    }

    pub fn wants_tick(&self) -> bool {
        self.waiting_for_response()
            || self.conversation.any_running()
            || !self.session.workspace_move_state().is_idle()
            || self.conversation.turn().is_compaction_active()
            || self.conversation.progress_indicator().is_active()
            || self.exit_state.is_confirming()
            || self.conversation.plan_tracker().has_completed_in_grace_period()
    }

    pub fn has_navigation(&self) -> bool {
        self.overlay.is_some() || self.route.is_fullscreen()
    }

    pub fn has_session_picker(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Sessions(_)))
    }

    pub fn has_modal(&self) -> bool {
        self.overlay.is_some()
    }

    pub fn full_screen_active(&self) -> bool {
        self.route.is_fullscreen()
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.session.workspace_move_state()
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_state == ExitState::Exiting
    }

    pub fn conversation_items(&self) -> &[ConversationItem] {
        self.conversation.items()
    }

    pub fn conversation_id(&self) -> crate::conversation::ConversationId {
        self.conversation.id()
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    pub(crate) fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn config_options(&self) -> &[LocalConfigOption] {
        self.session.config_options()
    }

    pub fn auth_methods(&self) -> &[acp::AuthMethod] {
        self.session.auth_methods()
    }

    pub(crate) fn content_padding(&self) -> usize {
        self.ui.content_padding
    }

    /// Everything the status line reads, gathered for one frame.
    pub fn status_line_model(&self) -> StatusLineModel<'_> {
        StatusLineModel {
            settings: &self.ui.status_line,
            config_options: self.session.config_options(),
            workspace: self.session.workspace_status(),
            agent_name: self.session.agent_name(),
            content_padding: self.ui.content_padding,
            context_usage: self.conversation.turn().context_usage(),
            unhealthy_servers: self.session.unhealthy_server_count(),
            waiting_for_response: self.waiting_for_response(),
            exit_confirmation: self.exit_state.is_confirming(),
        }
    }

    pub fn ui_settings(&self) -> &UiSettings {
        self.ui.settings.ui()
    }

    /// The active theme; the renderer resyncs when `theme_generation` moves.
    pub fn theme(&self) -> &Theme {
        &self.ui.theme
    }

    pub(crate) fn theme_generation(&self) -> Generation {
        self.ui.theme_generation
    }

    /// A prompt is outstanding, so the agent owes us a reply.
    pub fn waiting_for_response(&self) -> bool {
        self.conversation.turn().is_prompt_in_flight()
    }

    /// Either the prompt or one of its tool calls is still running.
    pub fn is_agent_busy(&self) -> bool {
        self.waiting_for_response() || self.conversation.any_running()
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        self.conversation.progress_indicator()
    }

    /// Test seam: the status line reads this through
    /// [`App::status_line_model`] rather than calling it.
    pub fn exit_confirmation_active(&self) -> bool {
        self.exit_state.is_confirming()
    }

    pub(crate) fn spinner_tick(&self) -> usize {
        self.conversation.turn().spinner_tick()
    }

    pub fn plan_entries(&self) -> Vec<acp::PlanEntry> {
        self.conversation.plan_tracker().current_entries()
    }

    /// Reaches past the renderer for the integration tests, which assert on the
    /// state a frame is drawn from rather than on the frame.
    pub fn has_plan(&self) -> bool {
        self.conversation.plan_tracker().has_entries()
    }

    /// Drops all conversation state atomically before starting a new session.
    fn reset_conversation(&mut self) {
        self.reset_turn_state();
        self.submission.reset();
        self.conversation.clear();
    }

    /// Clears the per-turn indicators that must not survive into a different
    /// conversation. Used on its own when a load lands, because the conversation
    /// was already cleared when that load was requested — and may since have
    /// gained notices the user still needs to see.
    fn reset_turn_state(&mut self) {
        // The spinner phase is cosmetic and survives, so a swap does not make
        // the indicator visibly jump.
        self.conversation.reset_feature_state();
    }

    fn refresh_progress(&mut self) {
        let override_phase = match self.session.workspace_move_state() {
            WorkspaceMoveState::Moving => Some(ProgressPhase::MovingWorkspace),
            WorkspaceMoveState::LoadingSession => Some(ProgressPhase::LoadingSession),
            _ if self.conversation.turn().is_compaction_active() => Some(ProgressPhase::Compacting),
            _ => None,
        };
        let interruptible = self.is_agent_busy();
        self.conversation.progress_indicator_mut().refresh(override_phase, interruptible);
    }

    fn return_to_conversation(&mut self) {
        self.close_overlay();
        self.route = Route::Conversation;
    }
}
