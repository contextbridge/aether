use crate::app::keybindings::Keybindings;
use crate::app::message::Message;
use crate::command::{Command, HostCommandResult};
use crate::conversation::plan_tracker::PlanTracker;
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressPhase};
use crate::conversation::status_line::{ContextUsageDisplay, StatusLineModel};
use crate::settings::{
    ResolvedStatusLineSettings, SettingsModel, UiSettings, resolve_content_padding, resolve_status_line_settings,
};
use crate::surfaces::composer::Composer;
use crate::theme::Theme;
use crate::view::generation::Generation;
use acp_utils::conversation::Activity;
use agent_client_protocol::schema::v2::PlanEntry;
use client_core::session_core::{ForegroundOperation, SessionCore, SessionCoreConfig};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::Instant;

pub mod message;
mod navigation;

pub use navigation::{Overlay, Route};

mod config;
mod effects;
mod input;
mod keybindings;
use config::build_theme_entries;
use input::CTRL_C_CONFIRM_WINDOW;

pub struct App {
    core: SessionCore,
    plan_tracker: PlanTracker,
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
}

pub struct AppConfig {
    pub session: SessionCoreConfig,
    pub settings: UiSettings,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExitState {
    #[default]
    Idle,
    Confirming(Instant),
    Exiting,
    ConnectionLost,
}

impl App {
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
            plan_tracker: PlanTracker::default(),
            ui,
            route: Route::Conversation,
            overlay: None,
            progress: ProgressIndicator::default(),
            spinner_tick: 0,
            composer: Composer::new(),
            exit_state: ExitState::Idle,
            swallow_next_click: false,
            commands: VecDeque::new(),
        };
        if let Some(error) = theme_error {
            app.update_core(|core| core.notify(&format!("Could not load selected theme: {error}")));
        }
        app.update_core(SessionCore::refresh_workspace_status);
        app
    }

    pub fn update(&mut self, message: Message) -> Vec<Command> {
        match message {
            Message::Terminal(event) => self.on_terminal_event(event),
            Message::Session(input) => self.update_core(|core| core.on_input(*input)),
            Message::Host(result) => self.on_host_result(*result),
            Message::Tick(now) => self.on_tick(now),
        }

        self.plan_tracker.sync(self.core.conversation().plan(), Instant::now());
        self.refresh_progress();
        self.take_commands()
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.commands.drain(..).collect()
    }

    pub fn core(&self) -> &SessionCore {
        &self.core
    }

    pub fn wants_tick(&self) -> bool {
        self.core.waiting_for_response()
            || matches!(
                self.core.foreground(),
                ForegroundOperation::ListingWorkspaces
                    | ForegroundOperation::PickingWorkspace
                    | ForegroundOperation::MovingWorkspace
                    | ForegroundOperation::ResumingSession { moved: true, .. }
            )
            || self.core.conversation().any_running()
            || self.core.conversation().activity() == Activity::Compacting
            || self.progress.is_active()
            || self.exit_state.is_confirming()
            || self.plan_tracker.has_completed_in_grace_period()
    }

    pub fn has_modal(&self) -> bool {
        self.overlay.is_some()
    }

    pub fn full_screen_active(&self) -> bool {
        self.route.is_fullscreen()
    }

    pub fn exit_result(&self) -> Option<Result<(), crate::error::AppError>> {
        match self.exit_state {
            ExitState::Exiting => Some(Ok(())),
            ExitState::ConnectionLost => Some(Err(crate::error::AppError::ConnectionLost)),
            ExitState::Idle | ExitState::Confirming(_) => None,
        }
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    pub(crate) fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub(crate) fn render_prompt_search(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if let Some(picker) = self.composer.prompt_search_mut() {
            picker.render(area, buf, theme, self.core.prompt_search());
        }
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
            waiting_for_response: self.core.waiting_for_response(),
            exit_confirmation: self.exit_state.is_confirming(),
        }
    }

    /// The active theme; the renderer resyncs when `theme_generation` moves.
    pub fn theme(&self) -> &Theme {
        &self.ui.theme
    }

    pub(crate) fn theme_generation(&self) -> Generation {
        self.ui.theme_generation
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        &self.progress
    }

    pub(crate) fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    pub fn plan_entries(&self) -> Vec<PlanEntry> {
        self.plan_tracker.current_entries()
    }

    fn on_tick(&mut self, now: Instant) {
        if let ExitState::Confirming(armed_at) = self.exit_state
            && now.duration_since(armed_at) > CTRL_C_CONFIRM_WINDOW
        {
            self.exit_state = ExitState::Idle;
        }
        if self.progress.is_active() {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
        self.progress.on_tick(now);
        self.plan_tracker.on_tick(now);
        if let Route::GitReview(screen) = &mut self.route {
            let outputs = screen.on_tick();
            self.dispatch_outputs(outputs);
        }
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
            HostCommandResult::BellFailed(error) => self.update_core(|core| core.report_error(&error)),
            HostCommandResult::UrlOpened { elicitation, result } => {
                if let Some(answer) =
                    self.elicitation_modal_mut(elicitation).and_then(|modal| modal.on_url_opened(result))
                {
                    self.answer_elicitation(elicitation, answer);
                }
            }
            HostCommandResult::UrlCopied { elicitation, result } => {
                if let Some(modal) = self.elicitation_modal_mut(elicitation) {
                    modal.on_url_copied(result);
                }
            }
            HostCommandResult::TaskFailed(error) => {
                self.update_core(|core| core.report_error(&format!("Background task failed: {error}")));
            }
        }
    }

    fn refresh_progress(&mut self) {
        let conversation = self.core.conversation();
        let override_phase = match self.core.foreground() {
            ForegroundOperation::MovingWorkspace => Some(ProgressPhase::MovingWorkspace),
            ForegroundOperation::ResumingSession { moved: true, .. } => Some(ProgressPhase::LoadingSession),
            _ => None,
        };
        let interruptible = self.core.is_agent_busy();
        self.progress.refresh(conversation, override_phase, interruptible);
    }
}

struct UiConfig {
    settings: SettingsModel,
    keybindings: Keybindings,
    content_padding: usize,
    status_line: ResolvedStatusLineSettings,
    theme: Theme,
    theme_generation: Generation,
}

impl ExitState {
    fn is_confirming(&self) -> bool {
        matches!(self, ExitState::Confirming(_))
    }
}
