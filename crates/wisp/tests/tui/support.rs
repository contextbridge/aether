pub(crate) use acp_utils::ElicitationSchema;
pub(crate) use acp_utils::client::AcpEvent;
pub(crate) use acp_utils::config_meta::SelectOptionMeta;
pub(crate) use acp_utils::config_option_id::ConfigOptionId;
pub(crate) use acp_utils::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, ElicitRequestParams, ElicitationAction,
    ElicitationParams, ElicitationResponse, McpNotification, McpServerAuthCapability, McpServerStatus,
    McpServerStatusEntry, SessionPreviewResponse, SessionPreviewRole, SessionPreviewTurn, SubAgentEvent,
    SubAgentProgressParams, SubAgentToolRequest, SubAgentToolResult, WorkspaceEntry, WorkspaceListResponse,
    WorkspaceMoveResponse,
};
pub(crate) use acp_utils::testing::test_connection;
pub(crate) use agent_client_protocol::schema::v1::{self as acp, SessionId};
pub(crate) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use ratatui::buffer::{Buffer, Cell};
pub(crate) use ratatui::layout::Position;
pub(crate) use ratatui::style::{Color, Modifier};
pub(crate) use std::fmt::Write as FmtWrite;
pub(crate) use std::io::Write as IoWrite;
pub(crate) use std::sync::Arc;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tempfile::TempDir;
pub(crate) use tokio::task::LocalSet;
pub(crate) use wisp::app::WorkspaceMoveState;
pub(crate) use wisp::command::{AgentCommand, Command, CommandResult, FailedCommand, FilesystemCommand};
pub(crate) use wisp::testing::{
    BackendEvent, CountingBackend, FakeGit, RecordingBackend, StreamContent, TestUi, TestUiBuilder, assert_buffer_eq,
    buffer_text, chunk_message, has_cell, line_text, row_containing, row_text, rows_with_background, session_update,
    text_chunk, thought_chunk, tool_completed,
};

pub(crate) use wisp::attachment::{AttachmentKind, PromptAttachment, build_attachments, classify_attachment};
pub(crate) use wisp::conversation::tool_calls::ToolStatus;
pub(crate) use wisp::conversation::{ConversationContent, ItemState};
pub(crate) use wisp::file_index::index_files;
pub(crate) use wisp::git_review::{FileStatus, GitDiffEvent, StageState};
pub(crate) use wisp::renderer::DrawContext;
pub(crate) use wisp::session::session_config_view::LocalConfigOption;
pub(crate) use wisp::settings::overlay::SettingsOverlay;
pub(crate) use wisp::settings::{StatusLineSegmentConfig, UiSettings};
pub(crate) use wisp::surfaces::composer::Composer;
pub(crate) use wisp::surfaces::input::UiEvent;
pub(crate) use wisp::surfaces::picker::CommandEntry;
pub(crate) use wisp::theme::Theme;
pub(crate) use wisp::view::generation::Generation;
pub(crate) use wisp::view::syntax::SyntaxHighlighter;

/// Runs `body` on a current-thread runtime inside a `LocalSet`: the ACP test
/// connection spawns `!Send` tasks, so they need a local set to live in.
pub(crate) fn block_on_local<F: std::future::Future>(body: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, body)
}

/// A form elicitation carrying `schema`, as an MCP server would send one.
pub(crate) fn form_elicitation(message: &str, schema: ElicitationSchema) -> ElicitationParams {
    ElicitationParams {
        server_name: "test".to_string(),
        request: ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: message.to_string(),
            requested_schema: schema,
        },
    }
}

/// Hands `ui` an elicitation request over a live test connection and returns the
/// channel the agent's answer comes back on. Must run inside [`block_on_local`].
pub(crate) async fn with_elicitation(
    ui: &mut TestUi,
    params: ElicitationParams,
) -> tokio::sync::oneshot::Receiver<ElicitationResponse> {
    let (cx, mut peer) = test_connection().await;
    let (responder, response_rx) = peer.fake_elicitation(&cx).await;
    ui.acp_event(AcpEvent::ElicitationRequest { params, responder });
    response_rx
}

pub(crate) fn message_texts(app: &TestUi) -> impl Iterator<Item = &str> {
    app.app().conversation_items().iter().filter_map(|item| match item.content() {
        ConversationContent::User(text) => Some(text.text.as_str()),
        ConversationContent::Notice(notice) => Some(notice.text.as_str()),
        _ => None,
    })
}

pub(crate) fn make_app() -> TestUi {
    TestUiBuilder::new().build()
}

pub(crate) fn make_app_in(working_dir: std::path::PathBuf) -> TestUi {
    TestUiBuilder::new().working_dir(working_dir).build()
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

/// Opens the `@` picker on `composer` and delivers the file index, the way the
/// event loop does once the walk completes.
pub(crate) fn open_file_picker(composer: &mut Composer, root: &std::path::Path) {
    let FilesystemCommand::IndexFiles { request_id, root } = composer.open_file_picker(root) else {
        panic!("opening the file picker should ask for a file index");
    };
    composer.on_files_indexed(request_id, index_files(&root));
}

pub(crate) fn assert_ctrl_c_exits(ui: &mut TestUi) {
    assert!(ui.app().has_navigation(), "test must open navigation before asserting the exit shortcut");
    ui.key(ctrl('c'));
    assert!(ui.app().exit_confirmation_active(), "first Ctrl+C should arm exit confirmation");
    assert!(!ui.app().exit_requested());
    ui.key(ctrl('c'));
    assert!(ui.app().exit_requested(), "second Ctrl+C should request exit");
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

pub(crate) fn tool_call(id: &str, title: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title)))
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

/// Renders the composer's completion list and reports whether `needle` shows up.
pub(crate) fn completion_contains(composer: &mut Composer, needle: &str) -> bool {
    use ratatui::widgets::StatefulWidget;
    let theme = Theme::default();
    let area = ratatui::layout::Rect::new(0, 0, 80, 8);
    let mut buffer = Buffer::empty(area);
    if let Some(overlay) = composer.completion_mut() {
        let (view, selection) = overlay.view(&theme);
        StatefulWidget::render(view, area, &mut buffer, selection);
    }
    buffer_text(&buffer).contains(needle)
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

pub(crate) fn make_app_with_session_preview() -> TestUi {
    TestUiBuilder::new().session_preview().build()
}

pub(crate) fn make_app_with_workspace_move() -> TestUi {
    TestUiBuilder::new().workspace_move().build()
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
