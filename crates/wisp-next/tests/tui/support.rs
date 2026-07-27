pub(crate) use acp_utils::ElicitationSchema;
pub(crate) use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
pub(crate) use acp_utils::config_meta::SelectOptionMeta;
pub(crate) use acp_utils::config_option_id::ConfigOptionId;
pub(crate) use acp_utils::notifications::{
    AetherCapabilities, AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams,
    CreateElicitationRequestParams, ElicitationAction, ElicitationParams, McpNotification, McpServerAuthCapability,
    McpServerStatus, McpServerStatusEntry, SessionPreviewResponse, SessionPreviewRole, SessionPreviewTurn,
    SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest, SubAgentToolResult, WorkspaceEntry,
    WorkspaceListResponse, WorkspaceMoveResponse,
};
pub(crate) use acp_utils::testing::test_connection;
pub(crate) use agent_client_protocol::schema::{self as acp, SessionId};
pub(crate) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
pub(crate) use ratatui::buffer::{Buffer, Cell};
pub(crate) use ratatui::layout::{Position, Size};
pub(crate) use ratatui::style::{Color, Modifier};
pub(crate) use ratatui::{Terminal, TerminalOptions, Viewport};
pub(crate) use std::fmt::Write as FmtWrite;
pub(crate) use std::io::Write as IoWrite;
pub(crate) use std::sync::Arc;
pub(crate) use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tempfile::TempDir;
pub(crate) use tokio::sync::mpsc::UnboundedReceiver;
pub(crate) use tokio::task::LocalSet;
pub(crate) use wisp_next::test_support::app::{App, AppConfig, HistoryItem, RuntimeEffect, WorkspaceMoveState};
pub(crate) use wisp_next::test_support::composer::Composer;
pub(crate) use wisp_next::test_support::generation::Generation;
pub(crate) use wisp_next::test_support::picker::CommandEntry;
pub(crate) use wisp_next::test_support::picker::index_files;
pub(crate) use wisp_next::test_support::presentation::{Presenter, Segment};
pub(crate) use wisp_next::test_support::render::sync_terminal as sync_terminal_with_renderer;
pub(crate) use wisp_next::test_support::render_context::RenderContext;
pub(crate) use wisp_next::test_support::screens::git_diff::GitDiffEvent;
pub(crate) use wisp_next::test_support::settings::{StatusLineSegmentConfig, UiSettings};
pub(crate) use wisp_next::test_support::settings_overlay::SettingsOverlay;
pub(crate) use wisp_next::test_support::syntax::SyntaxHighlighter;
pub(crate) use wisp_next::test_support::tasks::{Task, TaskResult};
pub(crate) use wisp_next::test_support::theme::Theme;
pub(crate) use wisp_next::test_support::tool_calls::ToolStatus;
pub(crate) use wisp_next::test_support::{inline_viewport_height, workspace_status::WorkspaceStatus};

/// Builds an `App` wired to a recording (or failable) prompt handle.
///
/// Defaults match the plain `make_app()` case; each method turns on exactly the
/// one thing a test cares about.
#[derive(Default)]
pub(crate) struct AppBuilder {
    working_dir: Option<std::path::PathBuf>,
    capabilities: AetherCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    session_capabilities: Option<acp::SessionCapabilities>,
}

impl AppBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn working_dir(mut self, working_dir: impl Into<std::path::PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    pub(crate) fn config_options(mut self, options: Vec<acp::SessionConfigOption>) -> Self {
        self.config_options = options;
        self
    }

    pub(crate) fn auth_methods(mut self, methods: Vec<acp::AuthMethod>) -> Self {
        self.auth_methods = methods;
        self
    }

    /// Overrides the capabilities wholesale, for tests that care about metadata
    /// the individual toggles do not cover.
    pub(crate) fn session_capabilities(mut self, capabilities: acp::SessionCapabilities) -> Self {
        self.session_capabilities = Some(capabilities);
        self
    }

    pub(crate) fn prompt_search(mut self) -> Self {
        self.capabilities.prompt_search = true;
        self
    }

    pub(crate) fn session_preview(mut self) -> Self {
        self.capabilities.session_preview = true;
        self
    }

    pub(crate) fn workspace_move(mut self) -> Self {
        self.capabilities.workspace_move = true;
        self
    }

    /// Builds against a handle that records every command it is given.
    pub(crate) fn build(self) -> (App, UnboundedReceiver<PromptCommand>) {
        let (prompt_handle, command_rx) = AcpPromptHandle::recording();
        (self.build_with(prompt_handle), command_rx)
    }

    /// Builds against a handle whose commands fail once the returned flag is set.
    pub(crate) fn build_failable(self) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
        let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
        (self.build_with(prompt_handle), fail_signal, command_rx)
    }

    fn build_with(self, prompt_handle: AcpPromptHandle) -> App {
        let session_capabilities = self
            .session_capabilities
            .unwrap_or_else(|| acp::SessionCapabilities::new().meta(Some(self.capabilities.to_meta())));
        App::new(AppConfig {
            session_id: SessionId::new("test-session"),
            agent_name: "aether".to_string(),
            prompt_capabilities: acp::PromptCapabilities::new(),
            session_capabilities,
            config_options: self.config_options,
            auth_methods: self.auth_methods,
            workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
            prompt_handle,
            working_dir: self.working_dir.unwrap_or_else(|| std::path::PathBuf::from(".")),
            settings: UiSettings::default(),
        })
    }
}

pub(crate) fn make_app() -> (App, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().build()
}

pub(crate) fn make_app_in(working_dir: std::path::PathBuf) -> (App, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().working_dir(working_dir).build()
}

pub(crate) fn select_option(id: &str, current_value: &str) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        id.to_string(),
        id.to_string(),
        current_value.to_string(),
        vec![acp::SessionConfigSelectOption::new(current_value.to_string(), current_value.to_string())],
    )
}

pub(crate) fn reasoning_option(current: &str, levels: &[&str]) -> acp::SessionConfigOption {
    let options: Vec<acp::SessionConfigSelectOption> =
        levels.iter().map(|&level| acp::SessionConfigSelectOption::new(level.to_string(), level.to_string())).collect();
    acp::SessionConfigOption::select("reasoning_effort", "Reasoning", current.to_string(), options)
}

pub(crate) fn mode_option(current: &str, modes: &[&str]) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), select_options(modes))
        .category(acp::SessionConfigOptionCategory::Mode)
}

/// A mode option whose values arrive under group headers rather than as a flat list.
pub(crate) fn grouped_mode_option(current: &str, groups: &[(&str, &[&str])]) -> acp::SessionConfigOption {
    let groups: Vec<acp::SessionConfigSelectGroup> = groups
        .iter()
        .map(|(name, modes)| {
            acp::SessionConfigSelectGroup::new((*name).to_string(), (*name).to_string(), select_options(modes))
        })
        .collect();
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), groups)
        .category(acp::SessionConfigOptionCategory::Mode)
}

fn select_options(values: &[&str]) -> Vec<acp::SessionConfigSelectOption> {
    values.iter().map(|&value| acp::SessionConfigSelectOption::new(value.to_string(), value.to_string())).collect()
}

pub(crate) fn make_terminal() -> Terminal<TestBackend> {
    make_terminal_with_width(40)
}

pub(crate) fn make_terminal_with_width(width: u16) -> Terminal<TestBackend> {
    let terminal_height = 15;
    Terminal::with_options(
        TestBackend::new(width, terminal_height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(terminal_height)) },
    )
    .unwrap()
}

pub(crate) fn make_terminal_tall() -> Terminal<TestBackend> {
    make_terminal_with_dimensions(80, 30)
}

pub(crate) fn make_terminal_with_dimensions(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::with_options(
        TestBackend::new(width, height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(height)) },
    )
    .unwrap()
}

/// Draws one frame with a throwaway presenter.
///
/// Only correct for a test that draws once: the presenter owns the committed
/// scrollback, so a fresh one per frame silently drops whatever earlier frames
/// committed. Tests that draw repeatedly use [`TestUi`].
pub(crate) fn sync_terminal(
    terminal: &mut Terminal<TestBackend>,
    app: &mut App,
) -> Result<(), std::convert::Infallible> {
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(terminal, app, &mut renderer)
}

/// A terminal and the presenter that owns its scrollback, for tests that draw
/// more than one frame.
pub(crate) struct TestUi {
    pub(crate) terminal: Terminal<TestBackend>,
    presenter: Presenter,
}

impl TestUi {
    pub(crate) fn new(terminal: Terminal<TestBackend>) -> Self {
        Self { terminal, presenter: Presenter::new(&UiSettings::default()) }
    }

    pub(crate) fn draw(&mut self, app: &mut App) {
        sync_terminal_with_renderer(&mut self.terminal, app, &mut self.presenter).unwrap();
    }
}

/// Opens the `@` picker on `composer` and delivers the file index, the way the
/// event loop does once the walk completes.
pub(crate) fn open_file_picker(composer: &mut Composer, root: &std::path::Path) {
    let Task::IndexFiles { request_id, root } = composer.open_file_picker(root) else {
        panic!("opening the file picker should ask for a file index");
    };
    composer.on_files_indexed(request_id, index_files(&root));
}

/// Runs whatever work the app has queued and feeds the results back, so a test
/// sees the state the event loop would have produced.
pub(crate) fn settle_tasks(app: &mut App) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    while let Some(effect) = app.take_effect() {
        if let RuntimeEffect::Spawn(task) = effect {
            let result = runtime.block_on(task.execute());
            app.on_task_result(result);
        }
    }
}

/// Whether the session picker is the open layer.
pub(crate) fn has_session_picker(app: &App) -> bool {
    app.has_session_picker()
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

pub(crate) fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

pub(crate) fn submit_prompt(app: &mut App, text: &str) {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
}

pub(crate) fn session_update(update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new("test-session"), update: Box::new(update) }
}

pub(crate) fn text_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

pub(crate) fn tool_call(id: &str, title: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title)))
}

pub(crate) fn tool_completed(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

pub(crate) fn tool_completed_with_diff(id: &str) -> AcpEvent {
    tool_completed_with_diff_contents(id, "fn old_name() {}\n", "fn new_name() {}\n")
}

pub(crate) fn tool_completed_with_diff_contents(id: &str, old: &str, new: &str) -> AcpEvent {
    let diff = acp::Diff::new("src/main.rs", new).old_text(old);
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Completed),
    )))
}

pub(crate) fn viewport_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let area = terminal.get_frame().area();
    let screen = terminal.backend().buffer();
    let mut viewport = Buffer::empty(ratatui::layout::Rect::new(0, 0, area.width, area.height));
    for y in 0..area.height {
        for x in 0..area.width {
            viewport[(x, y)] = screen[(area.x + x, area.y + y)].clone();
        }
    }
    viewport
}

pub(crate) fn history_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let viewport_area = terminal.get_frame().area();
    let screen = terminal.backend().buffer();
    let scrollback = terminal.backend().scrollback();
    let history_height = scrollback.area.height.saturating_add(viewport_area.top());
    let mut history = Buffer::empty(ratatui::layout::Rect::new(0, 0, screen.area.width, history_height));
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

pub(crate) fn conversation_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
    let history = history_buffer(terminal);
    let viewport = viewport_buffer(terminal);
    let mut conversation = Buffer::empty(ratatui::layout::Rect::new(
        0,
        0,
        viewport.area.width,
        history.area.height.saturating_add(viewport.area.height),
    ));
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

pub(crate) fn row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    (buffer.area.top()..buffer.area.bottom()).find(|&y| {
        let row = (buffer.area.left()..buffer.area.right())
            .map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol))
            .collect::<String>();
        row.contains(needle)
    })
}

pub(crate) fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", Cell::symbol));
        }
        out.push('\n');
    }
    out
}

pub(crate) fn has_cell(buffer: &Buffer, symbol: &str, predicate: impl Fn(&Cell) -> bool) -> bool {
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            if let Some(cell) = buffer.cell((x, y))
                && cell.symbol() == symbol
                && predicate(cell)
            {
                return true;
            }
        }
    }
    false
}

/// Renders the composer's completion list and reports whether `needle` shows up.
pub(crate) fn completion_contains(composer: &mut Composer, needle: &str) -> bool {
    use ratatui::widgets::Widget;
    let theme = Theme::default();
    let area = ratatui::layout::Rect::new(0, 0, 80, 8);
    let mut buffer = Buffer::empty(area);
    if let Some(overlay) = composer.completion() {
        overlay.view(&theme).render(area, &mut buffer);
    }
    buffer_text(&buffer).contains(needle)
}

pub(crate) fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

pub(crate) fn rows_with_background(buffer: &Buffer, background: ratatui::style::Color) -> usize {
    (buffer.area.top()..buffer.area.bottom())
        .filter(|&y| {
            (buffer.area.left()..buffer.area.right())
                .any(|x| buffer.cell((x, y)).is_some_and(|cell| cell.bg == background))
        })
        .count()
}

pub(crate) fn session_info(id: &str, cwd: &str, title: &str, updated_at: &str) -> acp::SessionInfo {
    acp::SessionInfo::new(SessionId::new(id), cwd)
        .title(Some(title.to_string()))
        .updated_at(Some(updated_at.to_string()))
}

pub(crate) fn sessions_listed(sessions: Vec<acp::SessionInfo>) -> AcpEvent {
    AcpEvent::SessionsListed { sessions }
}

pub(crate) fn session_loaded(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    AcpEvent::SessionLoaded { session_id: SessionId::new(session_id), config_options }
}

pub(crate) fn new_session_created(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    AcpEvent::NewSessionCreated { session_id: SessionId::new(session_id), config_options }
}

pub(crate) fn session_preview_response(session_id: &str) -> SessionPreviewResponse {
    SessionPreviewResponse {
        session_id: session_id.to_string(),
        cwd: std::path::PathBuf::from("/tmp/project"),
        created_at: "2025-01-01T00:00:00Z".to_string(),
        model: "claude".to_string(),
        selected_mode: Some("code".to_string()),
        transcript: vec![
            SessionPreviewTurn { role: SessionPreviewRole::User, text: "hello".to_string() },
            SessionPreviewTurn { role: SessionPreviewRole::Assistant, text: "hi there".to_string() },
        ],
        tool_call_count: 1,
        truncated: false,
    }
}

pub(crate) fn session_update_for(session_id: &str, update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new(session_id), update: Box::new(update) }
}

pub(crate) fn user_message_chunk(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))))
}

pub(crate) fn make_app_with_session_preview() -> (App, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().session_preview().build()
}

pub(crate) fn make_app_with_workspace_move() -> (App, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().workspace_move().build()
}

pub(crate) fn workspace_entry(path: &str, is_current: bool) -> WorkspaceEntry {
    WorkspaceEntry { path: std::path::PathBuf::from(path), is_current }
}

pub(crate) fn workspaces_listed(workspaces: Vec<WorkspaceEntry>) -> AcpEvent {
    AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces })
}

pub(crate) fn workspace_list_failed(error: &str) -> AcpEvent {
    AcpEvent::WorkspaceListFailed { error: error.to_string() }
}

pub(crate) fn workspace_moved(new_cwd: &str) -> AcpEvent {
    AcpEvent::WorkspaceMoved(WorkspaceMoveResponse { new_cwd: std::path::PathBuf::from(new_cwd) })
}

pub(crate) fn workspace_move_failed(error: &str) -> AcpEvent {
    AcpEvent::WorkspaceMoveFailed { error: error.to_string() }
}

pub(crate) fn make_failable_app_with_workspace_move() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().workspace_move().build_failable()
}
