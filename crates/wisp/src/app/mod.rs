use crate::app::keybindings::Keybindings;
use crate::app::message::Message;
use crate::command::{Command, CommandResult, HostCommandResult};
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressPhase};
use crate::conversation::status_line::{ContextUsageDisplay, StatusLineModel};
use crate::session::platform::{BrowserOpener, ClipboardWriter, default_browser_opener, default_clipboard_writer};
use crate::settings::{
    ResolvedStatusLineSettings, SettingsModel, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::surfaces::composer::Composer;
use crate::theme::Theme;
use crate::view::generation::Generation;
use acp_utils::client::AcpEvent;
use acp_utils::conversation::{ConversationId, ConversationItem};
use agent_client_protocol::schema::v2::PlanEntry;
use agent_client_protocol::schema::v2::{self as acp};
use client_core::session::Session;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::{ForegroundOperation, QueuedPrompt, SessionCore, SessionCoreConfig};
use std::collections::VecDeque;
use std::time::Instant;
use tokio::sync::mpsc;

pub mod message;
mod navigation;

pub use navigation::{Overlay, Route};

mod config;
mod effects;
mod input;
mod keybindings;
mod session;
mod submission;
use config::build_theme_entries;
use input::CTRL_C_CONFIRM_WINDOW;

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

/// Root UI state: feeds terminal input and ACP events through the session
/// core and turns its effects into the routes, overlays, and composer the
/// renderer draws each frame.
pub struct App {
    core: SessionCore,
    ui: UiConfig,
    route: Route,
    overlay: Option<Overlay>,
    progress: ProgressIndicator,
    spinner_tick: usize,
    composer: Composer,
    exit_state: ExitState,
    swallow_next_click: bool,
    /// What the event loop still owes the outside world.
    commands: VecDeque<Command>,
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
    pub session: SessionCoreConfig,
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
        session: Session,
        settings: UiSettings,
    ) -> (Self, mpsc::UnboundedReceiver<AcpEvent>, acp_utils::client::AcpClientHandle) {
        let Session { client, response, working_dir, workspace_status, workspace_access } = session;
        let mut app = Self::new(AppConfig {
            session: SessionCoreConfig {
                initialize_response: client.initialize_response,
                session_response: response,
                working_dir,
                workspace_status,
                workspace_access,
                client_name: crate::client_info().name,
            },
            settings,
            browser_opener: default_browser_opener(),
            clipboard_writer: default_clipboard_writer(),
        });
        let effects = app.core.refresh_workspace_status();
        app.apply_effects(effects);
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
        let mut app = Self {
            core: SessionCore::new(config.session),
            ui,
            route: Route::Conversation,
            overlay: None,
            progress: ProgressIndicator::default(),
            spinner_tick: 0,
            composer: Composer::new(),
            exit_state: ExitState::Idle,
            swallow_next_click: false,
            commands: VecDeque::new(),
            browser_opener: config.browser_opener,
            clipboard_writer: config.clipboard_writer,
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

    pub fn on_acp_event(&mut self, event: AcpEvent) {
        let effects = self.core.on_acp_event(event);
        self.apply_effects(effects);
    }

    pub fn on_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::Host(result) => self.on_host_result(result),
            result => {
                let effects = self.core.on_command_result(result);
                self.apply_effects(effects);
            }
        }
    }

    pub fn on_tick(&mut self, now: Instant) {
        if let ExitState::Confirming(armed_at) = self.exit_state
            && now.duration_since(armed_at) > CTRL_C_CONFIRM_WINDOW
        {
            self.exit_state = ExitState::Idle;
        }
        if self.progress.is_active() {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
        self.progress.on_tick(now);
        self.core.on_tick(now);
    }

    pub fn wants_tick(&self) -> bool {
        self.waiting_for_response()
            || matches!(
                self.core.foreground(),
                ForegroundOperation::ListingWorkspaces
                    | ForegroundOperation::PickingWorkspace
                    | ForegroundOperation::MovingWorkspace
                    | ForegroundOperation::LoadingWorkspaceSession { .. }
            )
            || self.core.conversation().any_running()
            || self.core.conversation().is_compacting()
            || self.progress.is_active()
            || self.exit_state.is_confirming()
            || self.core.plan_tracker().has_completed_in_grace_period()
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
        self.core.foreground()
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
        self.core.session().session_id()
    }

    pub fn conversation_items(&self) -> &[ConversationItem] {
        self.core.conversation().items()
    }

    pub fn conversation_id(&self) -> ConversationId {
        self.core.conversation().id()
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    pub(crate) fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn config_options(&self) -> &[LocalConfigOption] {
        self.core.session().config_options()
    }

    pub fn auth_methods(&self) -> &[acp::AuthMethod] {
        self.core.session().auth_methods()
    }

    pub(crate) fn content_padding(&self) -> usize {
        self.ui.content_padding
    }

    /// Everything the status line reads, gathered for one frame.
    pub fn status_line_model(&self) -> StatusLineModel<'_> {
        let session = self.core.session();
        StatusLineModel {
            settings: &self.ui.status_line,
            config_options: session.config_options(),
            workspace: session.workspace_status(),
            agent_name: session.agent_name(),
            content_padding: self.ui.content_padding,
            context_usage: self.core.conversation().context_usage().map(ContextUsageDisplay::from),
            unhealthy_servers: session.unhealthy_server_count(),
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

    pub fn waiting_for_response(&self) -> bool {
        self.core.waiting_for_response()
    }

    pub fn queued_prompts(&self) -> &[QueuedPrompt] {
        self.core.queued_prompts()
    }

    /// Either the prompt or one of its tool calls is still running.
    pub fn is_agent_busy(&self) -> bool {
        self.core.is_agent_busy()
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        &self.progress
    }

    /// Test seam: the status line reads this through
    /// [`App::status_line_model`] rather than calling it.
    pub fn exit_confirmation_active(&self) -> bool {
        self.exit_state.is_confirming()
    }

    pub(crate) fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    pub fn plan_entries(&self) -> Vec<PlanEntry> {
        self.core.plan_tracker().current_entries()
    }

    /// Reaches past the renderer for the integration tests, which assert on the
    /// state a frame is drawn from rather than on the frame.
    pub fn has_plan(&self) -> bool {
        self.core.plan_tracker().has_entries()
    }

    fn on_host_result(&mut self, result: HostCommandResult) {
        match result {
            HostCommandResult::ThemesListed(files) => {
                let entries = build_theme_entries(self.ui.settings.ui(), &files);
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.upsert_local_entries(entries);
                }
            }
            HostCommandResult::ReviewThemesListed(choices) => match &mut self.route {
                Route::GitReview(screen) => screen.set_theme_choices(choices),
                Route::ArtifactReview(screen) => screen.set_theme_choices(choices),
                Route::Conversation => {}
            },
            HostCommandResult::ThemeApplied(result) => self.finish_theme_change(result),
            HostCommandResult::TerminalFailed(error) => self.notify(&error),
        }
    }

    fn queue(&mut self, command: Command) {
        self.commands.push_back(command);
    }

    fn refresh_progress(&mut self) {
        let conversation = self.core.conversation();
        let override_phase = match self.core.foreground() {
            ForegroundOperation::MovingWorkspace => Some(ProgressPhase::MovingWorkspace),
            ForegroundOperation::LoadingWorkspaceSession { .. } => Some(ProgressPhase::LoadingSession),
            _ if conversation.is_compacting() => Some(ProgressPhase::Compacting),
            _ => None,
        };
        let interruptible = self.core.is_agent_busy();
        self.progress.refresh(conversation, override_phase, interruptible);
    }
}
