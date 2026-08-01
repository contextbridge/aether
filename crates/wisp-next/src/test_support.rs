//! Explicit facade for integration tests: the scenario harness that owns the
//! whole UI (app, renderer, terminal, command receiver) plus the narrow set of
//! production types tests assert against.
//!
//! Prefer [`TestUi`] over reaching for [`App`], [`Renderer`], or a raw
//! `Terminal<TestBackend>`: it owns all of them and routes input, ACP events,
//! task settling, and drawing through the same seams the event loop uses.

use crate::app::{App, AppConfig, RuntimeEffect};
use crate::renderer::Renderer;
use crate::settings::UiSettings;
use crate::terminal::inline_viewport_height;
use crate::theme::Theme;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

pub mod app {
    pub use crate::app::{App, AppConfig, HistoryItem, RuntimeEffect, WorkspaceMoveState};
}

pub mod attachments {
    pub use crate::attachments::{AttachmentKind, PromptAttachment, build_attachments, classify_attachment};
}

pub mod composer {
    pub use crate::composer::Composer;
}

pub mod elicitation {
    pub use crate::elicitation::ElicitationResponder;
}

pub mod filterable_list {
    pub use crate::filterable_list::FilterableList;
}

pub mod generation {
    pub use crate::generation::Generation;
}

pub mod git_diff {
    pub use crate::git_diff::{FileDiff, FileStatus, GitDiffDocument, Hunk, PatchLine, PatchLineKind, StageState};
}

pub mod picker {
    pub use crate::picker::{CommandEntry, index_files};
}

pub mod plan_review {
    pub use crate::plan_review::{PlanDocument, ReviewComment, compile_feedback};
}

pub mod progress_indicator {
    pub use crate::progress_indicator::SPINNER_FRAMES;
}

pub mod renderer {
    pub use crate::renderer::DrawContext;
}

pub mod screens {
    pub mod git_diff {
        pub use crate::screens::git_diff::{GitDiffEvent, GitDiffScreen};
    }

    pub mod plan_review {
        pub use crate::screens::plan_review::PlanReviewScreen;
    }
}

pub mod selection {
    pub use crate::selection::Direction;
}

pub mod session_config_view {
    pub use crate::session_config_view::SessionConfigView;
}

pub mod settings {
    pub use crate::settings::{StatusLineSegmentConfig, StatusLineSettings, StatusLineStyle, UiSettings};
}

pub mod settings_overlay {
    pub use crate::settings_overlay::SettingsOverlay;
}

pub mod surface {
    pub use crate::surface::{Action, MouseAction, Surface, UiEvent};
}

pub mod syntax {
    pub use crate::syntax::SyntaxHighlighter;
}

pub mod tasks {
    pub use crate::tasks::{Task, TaskResult};
}

pub mod theme {
    pub use crate::theme::Theme;
}

pub mod tool_calls {
    pub use crate::tool_calls::ToolStatus;
}

pub mod terminal {
    pub use crate::terminal::{inline_viewport_height, inline_viewport_needs_resync};
}

pub mod workspace_status {
    pub use crate::workspace_status::WorkspaceStatus;
}

/// A whole UI scenario: the app, the renderer that owns its scrollback, the
/// terminal it draws into, and the receiver for the commands the app sends.
///
/// Construct through [`TestUi::new`], [`TestUi::with_dimensions`], or
/// [`TestUiBuilder`]. Input, ACP events, ticks, and task results are routed the
/// same way the event loop routes them, and drawing happens against the same
/// renderer instance every frame so committed scrollback survives.
pub struct TestUi<B: Backend = TestBackend> {
    app: App,
    renderer: Renderer,
    terminal: Terminal<B>,
    command_rx: UnboundedReceiver<PromptCommand>,
}

impl<B: Backend> TestUi<B>
where
    B::Error: std::fmt::Debug,
{
    /// Builds a UI around an arbitrary backend (e.g. a recording backend that
    /// asserts on frame-level terminal commands). The app is the default
    /// scenario; configure anything else through [`TestUiBuilder`].
    pub fn with_backend(backend: B) -> Self {
        let (prompt_handle, command_rx) = AcpPromptHandle::recording();
        let app = App::new(TestUiBuilder::new().app_config(prompt_handle));
        Self { app, renderer: Renderer::new(&UiSettings::default()), terminal: test_terminal(backend), command_rx }
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// The receiver the app's prompt handle sends commands to.
    pub fn command_rx(&mut self) -> &mut UnboundedReceiver<PromptCommand> {
        &mut self.command_rx
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }

    /// The theme the renderer draws with.
    pub fn theme(&self) -> &Theme {
        self.renderer.theme()
    }

    /// Draws one frame, like the event loop does after every input batch.
    pub fn draw(&mut self) {
        self.renderer.draw(&mut self.terminal, &mut self.app).unwrap();
    }

    /// Routes a terminal event (key, paste, mouse) the way the event loop does.
    pub fn terminal_event(&mut self, event: Event) {
        self.app.on_terminal_event(event);
    }

    pub fn key(&mut self, key: KeyEvent) {
        self.app.on_key(key);
    }

    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    /// Types `text` and submits it with Enter.
    pub fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    pub fn paste(&mut self, text: &str) {
        self.app.on_paste(text);
    }

    pub fn acp_event(&mut self, event: AcpEvent) {
        self.app.on_acp_event(event);
    }

    pub fn tick(&mut self, now: Instant) {
        self.app.on_tick(now);
    }

    /// Runs whatever work the app has queued and feeds the results back, so a
    /// test sees the state the event loop would have produced.
    pub fn settle_tasks(&mut self) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        while let Some(effect) = self.app.take_effect() {
            if let RuntimeEffect::Spawn(task) = effect {
                let result = runtime.block_on(task.execute());
                self.app.on_task_result(result);
            }
        }
    }
}

impl TestUi<TestBackend> {
    /// The default scenario: 40x15 terminal, plain app, recording prompt handle.
    pub fn new() -> Self {
        Self::with_dimensions(40, 15)
    }

    pub fn with_dimensions(width: u16, height: u16) -> Self {
        TestUiBuilder::new().dimensions(width, height).build()
    }

    /// Resizes the backing terminal. The next [`Self::draw`] re-measures the
    /// inline viewport the way `Renderer::draw` does in the event loop.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
    }

    /// What the inline viewport currently shows: the composer, status line,
    /// and the live tail of the conversation.
    pub fn viewport(&mut self) -> Buffer {
        viewport_buffer(&mut self.terminal)
    }

    /// The terminal's own scrollback plus the rows the inline viewport leaves
    /// above itself: the committed conversation that left the live tail.
    pub fn history(&mut self) -> Buffer {
        history_buffer(&mut self.terminal)
    }

    /// [`Self::history`] stacked on [`Self::viewport`]: everything the
    /// conversation has shown, oldest at the top.
    pub fn conversation(&mut self) -> Buffer {
        conversation_buffer(&mut self.terminal)
    }

    pub fn viewport_text(&mut self) -> String {
        buffer_text(&self.viewport())
    }

    pub fn history_text(&mut self) -> String {
        buffer_text(&self.history())
    }

    pub fn conversation_text(&mut self) -> String {
        buffer_text(&self.conversation())
    }

    /// Row (within the viewport buffer) of the first line containing `needle`.
    pub fn viewport_row(&mut self, needle: &str) -> Option<u16> {
        row_containing(&self.viewport(), needle)
    }

    pub fn assert_viewport_contains(&mut self, needle: &str) {
        let viewport = self.viewport_text();
        assert!(
            viewport.contains(needle),
            "viewport should contain {needle:?}:
{viewport}"
        );
    }

    pub fn assert_viewport_not_contains(&mut self, needle: &str) {
        let viewport = self.viewport_text();
        assert!(
            !viewport.contains(needle),
            "viewport should not contain {needle:?}:
{viewport}"
        );
    }

    pub fn assert_history_contains(&mut self, needle: &str) {
        let history = self.history_text();
        assert!(
            history.contains(needle),
            "history should contain {needle:?}:
{history}"
        );
    }

    pub fn assert_history_not_contains(&mut self, needle: &str) {
        let history = self.history_text();
        assert!(
            !history.contains(needle),
            "history should not contain {needle:?}:
{history}"
        );
    }

    pub fn assert_conversation_contains(&mut self, needle: &str) {
        let conversation = self.conversation_text();
        assert!(
            conversation.contains(needle),
            "conversation should contain {needle:?}:
{conversation}"
        );
    }

    pub fn assert_conversation_not_contains(&mut self, needle: &str) {
        let conversation = self.conversation_text();
        assert!(
            !conversation.contains(needle),
            "conversation should not contain {needle:?}:
{conversation}"
        );
    }

    /// Asserts the viewport's visible text matches `expected` row-by-row.
    pub fn assert_viewport<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.viewport(), expected);
    }

    /// Asserts the committed history's visible text matches `expected` row-by-row.
    pub fn assert_history<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.history(), expected);
    }

    /// Asserts the stitched conversation's visible text matches `expected` row-by-row.
    pub fn assert_conversation<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.conversation(), expected);
    }
}

impl Default for TestUi<TestBackend> {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a [`TestUi`]: the terminal dimensions plus every app-scenario option
/// a test cares about. Defaults match a plain `make_app()`-style scenario.
pub struct TestUiBuilder {
    width: u16,
    height: u16,
    working_dir: Option<PathBuf>,
    capabilities: AetherCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    session_capabilities: Option<acp::SessionCapabilities>,
    settings: UiSettings,
    workspace_status: Option<WorkspaceStatus>,
}

impl Default for TestUiBuilder {
    fn default() -> Self {
        Self {
            width: 40,
            height: 15,
            working_dir: None,
            capabilities: AetherCapabilities::default(),
            config_options: Vec::new(),
            auth_methods: Vec::new(),
            session_capabilities: None,
            settings: UiSettings::default(),
            workspace_status: None,
        }
    }
}

impl TestUiBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dimensions(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    pub fn config_options(mut self, options: Vec<acp::SessionConfigOption>) -> Self {
        self.config_options = options;
        self
    }

    pub fn auth_methods(mut self, methods: Vec<acp::AuthMethod>) -> Self {
        self.auth_methods = methods;
        self
    }

    pub fn settings(mut self, settings: UiSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn workspace_status(mut self, workspace_status: WorkspaceStatus) -> Self {
        self.workspace_status = Some(workspace_status);
        self
    }

    /// Overrides the capabilities wholesale, for tests that care about metadata
    /// the individual toggles do not cover.
    pub fn session_capabilities(mut self, capabilities: acp::SessionCapabilities) -> Self {
        self.session_capabilities = Some(capabilities);
        self
    }

    pub fn prompt_search(mut self) -> Self {
        self.capabilities.prompt_search = true;
        self
    }

    pub fn session_preview(mut self) -> Self {
        self.capabilities.session_preview = true;
        self
    }

    pub fn workspace_move(mut self) -> Self {
        self.capabilities.workspace_move = true;
        self
    }

    /// Builds the whole UI scenario.
    pub fn build(self) -> TestUi {
        let (prompt_handle, command_rx) = AcpPromptHandle::recording();
        self.finish(prompt_handle, command_rx)
    }

    /// Builds against a handle whose commands fail once the returned flag is set.
    pub fn build_failable(self) -> (TestUi, Arc<AtomicBool>) {
        let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
        (self.finish(prompt_handle, command_rx), fail_signal)
    }

    /// The app alone, for tests that never draw.
    pub fn build_app(self) -> (App, UnboundedReceiver<PromptCommand>) {
        let (prompt_handle, command_rx) = AcpPromptHandle::recording();
        let ui = self.finish(prompt_handle, command_rx);
        (ui.app, ui.command_rx)
    }

    /// The app alone against a failable handle, for tests that never draw.
    pub fn build_app_failable(self) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
        let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
        let ui = self.finish(prompt_handle, command_rx);
        (ui.app, fail_signal, ui.command_rx)
    }

    fn finish(self, prompt_handle: AcpPromptHandle, command_rx: UnboundedReceiver<PromptCommand>) -> TestUi {
        TestUi {
            app: App::new(self.app_config(prompt_handle)),
            renderer: Renderer::new(&self.settings),
            terminal: test_terminal(TestBackend::new(self.width, self.height)),
            command_rx,
        }
    }

    fn app_config(&self, prompt_handle: AcpPromptHandle) -> AppConfig {
        let session_capabilities = self
            .session_capabilities
            .clone()
            .unwrap_or_else(|| acp::SessionCapabilities::new().meta(Some(self.capabilities.clone().to_meta())));
        AppConfig {
            session_id: SessionId::new("test-session"),
            agent_name: "aether".to_string(),
            prompt_capabilities: acp::PromptCapabilities::new(),
            session_capabilities,
            config_options: self.config_options.clone(),
            auth_methods: self.auth_methods.clone(),
            workspace_status: self
                .workspace_status
                .clone()
                .unwrap_or_else(|| WorkspaceStatus::new("~/code/demo", Some("main".to_string()))),
            prompt_handle,
            working_dir: self.working_dir.clone().unwrap_or_else(|| PathBuf::from(".")),
            settings: self.settings.clone(),
        }
    }
}

/// The terminal a scenario draws into: the inline viewport sized from the
/// backend, exactly as the real event loop enters it.
fn test_terminal<B: Backend>(backend: B) -> Terminal<B>
where
    B::Error: std::fmt::Debug,
{
    let height = backend.size().unwrap().height;
    Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(height)) })
        .unwrap()
}

/// What the inline viewport shows: `terminal.get_frame().area()` clipped out of
/// the backend's full screen buffer.
fn viewport_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let area = terminal.get_frame().area();
    let screen = terminal.backend().buffer();
    let mut viewport = Buffer::empty(Rect::new(0, 0, area.width, area.height));
    for y in 0..area.height {
        for x in 0..area.width {
            viewport[(x, y)] = screen[(area.x + x, area.y + y)].clone();
        }
    }
    viewport
}

/// Content Ratatui's `insert_before` committed above the inline viewport.
fn history_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let viewport_area = terminal.get_frame().area();
    let screen = terminal.backend().buffer();
    let scrollback = terminal.backend().scrollback();
    let history_height = scrollback.area.height.saturating_add(viewport_area.top());
    let mut history = Buffer::empty(Rect::new(0, 0, screen.area.width, history_height));
    for y in 0..scrollback.area.height {
        for x in 0..scrollback.area.width {
            history[(x, y)] = scrollback[(x, y)].clone();
        }
    }
    for y in 0..viewport_area.top() {
        for x in 0..screen.area.width {
            history[(x, scrollback.area.height + y)] = screen[(x, y)].clone();
        }
    }
    history
}

fn conversation_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let history = history_buffer(terminal);
    let viewport = viewport_buffer(terminal);
    let mut conversation =
        Buffer::empty(Rect::new(0, 0, viewport.area.width, history.area.height.saturating_add(viewport.area.height)));
    for y in 0..history.area.height {
        for x in 0..history.area.width {
            conversation[(x, y)] = history[(x, y)].clone();
        }
    }
    for y in 0..viewport.area.height {
        for x in 0..viewport.area.width {
            conversation[(x, history.area.height + y)] = viewport[(x, y)].clone();
        }
    }
    conversation
}

fn row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    (buffer.area.top()..buffer.area.bottom()).find(|&y| {
        let row = (buffer.area.left()..buffer.area.right())
            .map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol))
            .collect::<String>();
        row.contains(needle)
    })
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", Cell::symbol));
        }
        out.push('\n');
    }
    out
}

/// Asserts `buffer`'s visible text matches `expected` row-by-row after trimming
/// trailing spaces, panicking on the first mismatched line with the full buffer
/// dumped.
fn assert_buffer_eq<S: AsRef<str>>(buffer: &Buffer, expected: &[S]) {
    let actual_lines: Vec<String> =
        (buffer.area.top()..buffer.area.bottom()).map(|y| row_text(buffer, y).trim_end().to_string()).collect();
    for index in 0..actual_lines.len().max(expected.len()) {
        let actual_line = actual_lines.get(index).map_or("", String::as_str);
        let expected_line = expected.get(index).map_or("", AsRef::as_ref).trim_end();
        assert_eq!(
            actual_line,
            expected_line,
            "line {index} mismatch:\n  expected: {expected_line:?}\n  actual:   {actual_line:?}\n\nfull buffer:\n{}",
            actual_lines.join("\n")
        );
    }
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (buffer.area.left()..buffer.area.right()).map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol)).collect()
}
