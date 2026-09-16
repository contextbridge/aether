use crate::Session;
use crate::app::keybindings::Keybindings;
use crate::app::message::Message;
use crate::command::{AgentCommand, Command, CommandResult};
use crate::conversation::items::{Conversation, ConversationItem};
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressPhase};
use crate::conversation::status_line::StatusLineModel;
use crate::conversation::tool_calls::ToolStatus;
use crate::session::WorkspaceAccess;
use crate::session::platform::{BrowserOpener, ClipboardWriter, default_browser_opener, default_clipboard_writer};
use crate::session::session_config_view::LocalConfigOption;
use crate::session::session_model::SessionModel;
use crate::session::workspace_status::WorkspaceStatus;
use crate::settings::{
    ResolvedStatusLineSettings, SettingsModel, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::surfaces::composer::Composer;
use crate::surfaces::picker::CommandEntry;
use crate::surfaces::workspace_picker::WorkspacePicker;
use crate::theme::Theme;
use crate::view::generation::Generation;
use acp_utils::client::AcpEvent;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::v2::PlanEntry;
use agent_client_protocol::schema::v2::{self as acp};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc;

pub mod message;
mod navigation;

pub use navigation::{Overlay, Route};

mod acp_reducer;
mod config;
mod foreground;
mod input;
mod keybindings;
mod session;
mod submission;
use config::build_theme_entries;
pub use foreground::{ForegroundOperation, PromptPhase};
use input::CTRL_C_CONFIRM_WINDOW;
use session::builtin_commands;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExitState {
    #[default]
    Idle,
    Confirming(Instant),
    Exiting,
    ConnectionLost,
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
    foreground: ForegroundOperation,
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
    pub initialize_response: acp::InitializeResponse,
    pub session_response: acp::NewSessionResponse,
    pub workspace_status: WorkspaceStatus,
    pub workspace_access: WorkspaceAccess,
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
        let Session { client, response, working_dir, workspace_status, workspace_access } = session;
        let mut app = Self::new(AppConfig {
            initialize_response: client.initialize_response,
            session_response: response,
            workspace_status,
            workspace_access,
            working_dir,
            settings,
            browser_opener: default_browser_opener(),
            clipboard_writer: default_clipboard_writer(),
        });
        app.resolve_workspace(app.session.working_dir().to_path_buf());
        (app, client.event_rx, client.handle)
    }

    pub fn new(config: AppConfig) -> Self {
        let (theme, theme_error) = match Theme::load_selection(&config.settings.theme) {
            Ok(theme) => (theme, None),
            Err(error) => (Theme::default(), Some(error)),
        };
        let ui = UiConfig {
            content_padding: resolve_content_padding(&config.settings),
            status_line: resolve_status_line_settings(&config.settings),
            keybindings: Keybindings::from_settings(&config.settings),
            theme,
            theme_generation: Generation::default(),
            settings: SettingsModel::new(config.settings.clone()),
        };
        let capabilities = AetherCapabilities::from_meta(
            config.initialize_response.capabilities.session.as_ref().and_then(|session| session.meta.as_ref()),
        );
        let initial_commands = builtin_commands(&capabilities);
        let browser_opener = config.browser_opener.clone();
        let clipboard_writer = config.clipboard_writer.clone();
        let mut app = Self {
            session: SessionModel::from_config(config, capabilities),
            ui,
            available_commands: initial_commands,
            route: Route::Conversation,
            overlay: None,
            conversation: Conversation::default(),
            composer: Composer::new(),
            exit_state: ExitState::Idle,
            commands: VecDeque::new(),
            foreground: ForegroundOperation::Idle,
            browser_opener,
            clipboard_writer,
        };
        if let Some(error) = theme_error {
            app.notify(&format!("Could not load selected theme: {error}"));
        }
        app
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

    #[allow(clippy::too_many_lines)]
    pub fn on_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::Prompt(Ok(_)) => self.foreground.accept_prompt(),
            CommandResult::Prompt(Err(error)) => {
                if self.waiting_for_response() {
                    self.finish_prompt(&ToolStatus::Error(format!("failed: {error}")));
                }
                self.foreground.reject_prompt();
                self.notify(&format!("Failed to send prompt: {error}"));
            }
            CommandResult::Cancel(result) | CommandResult::AuthenticateMcp(result) => {
                if let Err(error) = result {
                    self.notify(&error);
                }
            }
            CommandResult::NewSession(result) => match result {
                Ok(response) => self.on_new_session(response.session_id, response.config_options),
                Err(error) => {
                    self.foreground = ForegroundOperation::Idle;
                    self.notify(&format!("Failed to create new session: {error}"));
                }
            },
            CommandResult::ResumeSession { session_id, result } => {
                if !matches!(&self.foreground,
                    ForegroundOperation::ResumingSession { session_id: expected, .. }
                    | ForegroundOperation::LoadingWorkspaceSession { session_id: expected, .. } if expected == &session_id)
                {
                    return;
                }
                match result {
                    Ok(response) => self.on_resumed_session(&session_id, response),
                    Err(error) => {
                        self.foreground = ForegroundOperation::Idle;
                        self.notify(&format!("Failed to resume session: {error}"));
                    }
                }
            }
            CommandResult::ConfigOptionsUpdated { conversation_id, result: Ok(response) } => {
                if conversation_id != self.conversation_id() {
                    return;
                }
                self.session.update_config_options(response.config_options);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.update_config_options(self.session.config_options());
                }
            }
            CommandResult::ConfigOptionsUpdated { conversation_id, result: Err(error) } => {
                if conversation_id != self.conversation_id() {
                    return;
                }
                tracing::warn!("set_session_config_option failed: {error}");
                self.notify(&format!("Failed to update setting: {error}"));
            }
            CommandResult::AuthenticationCompleted { method_id, result: Ok(_) } => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_complete(&method_id);
                }
            }
            CommandResult::AuthenticationCompleted { method_id, result: Err(error) } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.on_authenticate_failed(&method_id);
                }
            }
            CommandResult::FilesIndexed { request_id, files } => self.composer.on_files_indexed(request_id, files),
            CommandResult::GitDiff(event) => {
                if let Route::GitReview(screen) = &mut self.route {
                    screen.on_event(event);
                }
            }
            CommandResult::GitWatchStarted { .. } => {}
            CommandResult::GitWatch(event) => {
                if let Route::GitReview(screen) = &mut self.route {
                    screen.on_watch_event(event);
                }
            }
            CommandResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            CommandResult::ThemesListed(files) => {
                let entries = build_theme_entries(self.ui.settings.ui(), &files);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.upsert_local_entries(entries);
                }
            }
            CommandResult::ReviewThemesListed(choices) => match &mut self.route {
                Route::GitReview(screen) => screen.set_theme_choices(choices),
                Route::ArtifactReview(screen) => screen.set_theme_choices(choices),
                Route::Conversation => {}
            },
            CommandResult::ThemeApplied(result) => self.finish_theme_change(result),
            CommandResult::WorkspaceResolved { cwd, status } => {
                if self.session.working_dir() == cwd {
                    self.session.set_workspace_status(status);
                }
            }
            CommandResult::SessionsListed(Ok(response)) => self.open_session_picker(response.sessions),
            CommandResult::SessionsListed(Err(error)) => self.notify(&format!("Failed to list sessions: {error}")),
            CommandResult::PromptSearchResults { result: Ok(response), .. } => {
                self.composer.prompt_search_on_results(response);
            }
            CommandResult::PromptSearchResults { query, result: Err(error) } => {
                if let Some(picker) = self.composer.prompt_search_mut() {
                    picker.on_failed(&query, error);
                }
            }
            CommandResult::SessionPreviewLoaded { result: Ok(preview), .. } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_loaded(preview);
                }
            }
            CommandResult::SessionPreviewLoaded { session_id, result: Err(error) } => {
                if let Some(Overlay::Sessions(picker)) = self.overlay.as_mut() {
                    picker.on_preview_failed(&session_id, error);
                }
            }
            CommandResult::WorkspacesListed(Ok(response)) => {
                self.open_overlay(Overlay::Workspaces(WorkspacePicker::new(
                    response.workspaces,
                    self.session.workspace_access(),
                )));
                self.foreground = ForegroundOperation::PickingWorkspace;
            }
            CommandResult::WorkspacesListed(Err(error)) => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            CommandResult::WorkspaceMoved(Ok(response)) => self.on_workspace_moved(response.new_cwd),
            CommandResult::WorkspaceMoved(Err(error)) => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            CommandResult::BackgroundFailed(error) | CommandResult::TerminalFailed(error) => self.notify(&error),
        }
    }

    fn start_prompt(&mut self, text: String, content: Option<Vec<acp::ContentBlock>>) {
        self.foreground = ForegroundOperation::Prompt(PromptPhase::Submitting);
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

    /// Queues a config change for the agent and session the app is attached to.
    fn set_config_option(&mut self, config_id: &str, value: &str) {
        self.queue(Command::Agent(AgentCommand::SetConfigOption {
            conversation_id: self.conversation_id(),
            session_id: self.session.session_id().clone(),
            config_id: config_id.to_string(),
            value: value.into(),
        }));
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
            || matches!(
                self.foreground,
                ForegroundOperation::ListingWorkspaces
                    | ForegroundOperation::PickingWorkspace
                    | ForegroundOperation::MovingWorkspace
                    | ForegroundOperation::LoadingWorkspaceSession { .. }
            )
            || self.conversation.any_running()
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

    pub fn foreground_operation(&self) -> &ForegroundOperation {
        &self.foreground
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_result().is_some()
    }

    pub fn exit_result(&self) -> Option<Result<(), crate::error::AppError>> {
        match self.exit_state {
            ExitState::Exiting => Some(Ok(())),
            ExitState::ConnectionLost => Some(Err(crate::error::AppError::ConnectionLost)),
            ExitState::Idle | ExitState::Confirming(_) => None,
        }
    }

    pub fn session_id(&self) -> &acp::SessionId {
        self.session.session_id()
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
        self.foreground.prompt_in_flight()
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

    pub fn plan_entries(&self) -> Vec<PlanEntry> {
        self.conversation.plan_tracker().current_entries()
    }

    /// Reaches past the renderer for the integration tests, which assert on the
    /// state a frame is drawn from rather than on the frame.
    pub fn has_plan(&self) -> bool {
        self.conversation.plan_tracker().has_entries()
    }

    /// Drops all conversation state atomically before starting a new session.
    fn reset_conversation(&mut self) {
        // The spinner phase is cosmetic and survives, so a swap does not make
        // the indicator visibly jump.
        self.conversation.reset_feature_state();
        self.foreground.clear_conversation();
        self.conversation.clear();
    }

    fn refresh_progress(&mut self) {
        let override_phase = match self.foreground {
            ForegroundOperation::MovingWorkspace => Some(ProgressPhase::MovingWorkspace),
            ForegroundOperation::LoadingWorkspaceSession { .. } => Some(ProgressPhase::LoadingSession),
            _ if self.conversation.turn().is_compaction_active() => Some(ProgressPhase::Compacting),
            _ => None,
        };
        let interruptible = self.is_agent_busy();
        self.conversation.progress_indicator_mut().refresh(override_phase, interruptible);
    }

    fn return_to_conversation(&mut self) {
        self.open_route(Route::Conversation);
    }
}
