pub(crate) use acp_utils::ElicitationSchema;
pub(crate) use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
pub(crate) use acp_utils::config_meta::SelectOptionMeta;
pub(crate) use acp_utils::config_option_id::ConfigOptionId;
pub(crate) use acp_utils::notifications::{
    AetherCapabilities, AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, ContextUsage,
    ContextUsageParams, CreateElicitationRequestParams, ElicitationAction, ElicitationParams, McpNotification,
    McpServerAuthCapability, McpServerStatus, McpServerStatusEntry, SessionPreviewResponse, SessionPreviewRole,
    SessionPreviewTurn, SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest, SubAgentToolResult, WorkspaceEntry,
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
pub(crate) use wisp_next::app::{App, AppConfig, HistoryItem};
pub(crate) use wisp_next::composer::Composer;
pub(crate) use wisp_next::picker::CommandEntry;
pub(crate) use wisp_next::presentation::TranscriptRenderer;
pub(crate) use wisp_next::render::sync_terminal as sync_terminal_with_renderer;
pub(crate) use wisp_next::screens::git_diff::GitDiffEvent;
pub(crate) use wisp_next::settings::UiSettings;
pub(crate) use wisp_next::theme::Theme;
pub(crate) use wisp_next::{inline_viewport_height, workspace_status::WorkspaceStatus};

pub(crate) fn make_app() -> (App, UnboundedReceiver<PromptCommand>) {
    make_app_in(std::path::PathBuf::from("."))
}

pub(crate) fn make_app_in(working_dir: std::path::PathBuf) -> (App, UnboundedReceiver<PromptCommand>) {
    make_app_with_metadata(working_dir, acp::SessionCapabilities::new(), Vec::new(), Vec::new())
}

pub(crate) fn make_app_with_metadata(
    working_dir: std::path::PathBuf,
    session_capabilities: acp::SessionCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = build_app_with_handle(working_dir, session_capabilities, config_options, auth_methods, prompt_handle);
    (app, command_rx)
}

pub(crate) fn make_failable_app() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app = build_app_with_handle(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        Vec::new(),
        prompt_handle,
    );
    (app, fail_signal, command_rx)
}

pub(crate) fn make_failable_app_with_prompt_search() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: true, session_preview: false, workspace_move: false }.to_meta(),
    ));
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app = build_app_with_handle(
        std::path::PathBuf::from("."),
        session_capabilities,
        Vec::new(),
        Vec::new(),
        prompt_handle,
    );
    (app, fail_signal, command_rx)
}

pub(crate) fn build_app_with_handle(
    working_dir: std::path::PathBuf,
    session_capabilities: acp::SessionCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    prompt_handle: AcpPromptHandle,
) -> App {
    App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities: acp::PromptCapabilities::new(),
        session_capabilities,
        config_options,
        auth_methods,
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir,
        settings: UiSettings::default(),
    })
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
    let options: Vec<acp::SessionConfigSelectOption> =
        modes.iter().map(|&mode| acp::SessionConfigSelectOption::new(mode.to_string(), mode.to_string())).collect();
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), options)
        .category(acp::SessionConfigOptionCategory::Mode)
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

pub(crate) fn sync_terminal(
    terminal: &mut Terminal<TestBackend>,
    app: &mut App,
) -> Result<(), std::convert::Infallible> {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(terminal, app, &mut renderer)
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
        overlay.view(&theme, area.width).render(area, &mut buffer);
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
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: false, session_preview: true, workspace_move: false }.to_meta(),
    ));
    make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new())
}

pub(crate) fn make_app_with_workspace_move() -> (App, UnboundedReceiver<PromptCommand>) {
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: false, session_preview: false, workspace_move: true }.to_meta(),
    ));
    make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new())
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
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: false, session_preview: false, workspace_move: true }.to_meta(),
    ));
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app = build_app_with_handle(
        std::path::PathBuf::from("."),
        session_capabilities,
        Vec::new(),
        Vec::new(),
        prompt_handle,
    );
    (app, fail_signal, command_rx)
}
