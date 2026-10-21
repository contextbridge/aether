pub(crate) use acp_utils::client::AcpEvent;
pub(crate) use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
pub(crate) use acp_utils::config_option_id::ConfigOptionId;
pub(crate) use acp_utils::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, McpNotification, McpServerAuthCapability, McpServerStatus,
    McpServerStatusEntry, PromptSearchResponse, PromptSearchResult, SessionPreviewResponse, SessionPreviewRole,
    SessionPreviewTurn, SubAgentEvent, SubAgentProgressParams, WorkspaceEntry, WorkspaceListResponse,
    WorkspaceMoveResponse, WorkspaceMoveTarget,
};
pub(crate) use acp_utils::testing::test_connection;
pub(crate) use agent_client_protocol::schema::v2::{
    self as acp, BooleanPropertySchema, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ElicitationUrlMode, SessionId,
    StringPropertySchema,
};
pub(crate) use client_core::command::{AgentCommand, FilesystemCommand, PromptRejection};
pub(crate) use client_core::command_entry::CommandEntry;
pub(crate) use client_core::session_config_view::LocalConfigOption;
pub(crate) use client_core::session_core::ForegroundOperation;
pub(crate) use client_core::testing::{FakeFilesystem, FakeGit};
pub(crate) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use ratatui::buffer::{Buffer, Cell};
pub(crate) use ratatui::layout::Position;
pub(crate) use ratatui::style::{Color, Modifier};
use std::borrow::Cow;
pub(crate) use std::fmt::Write as FmtWrite;
pub(crate) use std::io::Write as IoWrite;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::Arc;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tokio::task::LocalSet;
pub(crate) use unicode_width::UnicodeWidthStr;
pub(crate) use utils::artifact_review::ArtifactReviewElicitationMeta;
pub(crate) use utils::testing::TestWorkspace;
pub(crate) use wisp::command::{Command, CommandResult, HostCommand, HostCommandResult};
pub(crate) use wisp::testing::{
    BackendEvent, InstrumentedBackend, StreamContent, TestUi, TestUiBuilder, assert_buffer_eq, buffer_text,
    chunk_message, compaction_update, context_cleared, git_diff_screen, has_cell, line_text, numbered_lines,
    render_surface, row_cells, row_containing, row_text, rows_with_background, session_update, sub_agent_progress,
    sub_agent_tool_update, text_chunk, text_chunk_with_id, text_diff, thought_chunk, thought_chunk_with_id,
    tool_completed, tool_status,
};

pub(crate) use acp_utils::conversation::{ConversationContent, ConversationItem, ItemState, ToolStatus};
pub(crate) use wisp::git_review::{FileStatus, StageState};
pub(crate) use wisp::settings::overlay::SettingsOverlay;
pub(crate) use wisp::settings::{StatusLineSegmentConfig, UiSettings};
pub(crate) use wisp::surfaces::composer::Composer;
pub(crate) use wisp::theme::Theme;
pub(crate) use wisp::view::input::UiEvent;
pub(crate) use wisp::view::syntax::SyntaxHighlighter;

pub(crate) type Step = fn(&mut TestUi);

pub(crate) fn block_on_local<F: std::future::Future>(body: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, body)
}

pub(crate) fn form_elicitation(
    server_name: &str,
    message: &str,
    schema: ElicitationSchema,
) -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), schema),
        message,
    )
    .meta(server_meta(server_name))
}

pub(crate) fn url_elicitation(server_name: &str, url: &str, elicitation_id: &str) -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationUrlMode::new(ElicitationSessionScope::new("test-session"), elicitation_id.to_string(), url),
        "Open this URL to authorize MCP server access.",
    )
    .meta(server_meta(server_name))
}

fn server_meta(server_name: &str) -> acp::Meta {
    acp::Meta::from_iter([(
        acp_utils::notifications::AETHER_META_NAMESPACE.to_string(),
        serde_json::json!({ "mcpServer": server_name }),
    )])
}

pub(crate) async fn with_elicitation(
    ui: &mut TestUi,
    params: CreateElicitationRequest,
) -> tokio::sync::oneshot::Receiver<CreateElicitationResponse> {
    let (cx, mut peer) = test_connection().await;
    let (responder, response_rx) = peer.fake_elicitation(&cx).await;
    ui.acp_event(AcpEvent::ElicitationRequest { params: Box::new(params), responder });
    response_rx
}

pub(crate) fn accepted_content(response: &CreateElicitationResponse) -> serde_json::Value {
    let ElicitationAction::Accept(accept) = &response.action else {
        panic!("expected an accepted elicitation, got {:?}", response.action);
    };
    match &accept.content {
        Some(content) => serde_json::to_value(content).expect("elicitation content is JSON-serializable"),
        None => serde_json::Value::Null,
    }
}

pub(crate) fn message_texts(app: &TestUi) -> impl Iterator<Item = String> + '_ {
    app.app().conversation_items().iter().filter_map(|item| match item.content() {
        ConversationContent::User(_) | ConversationContent::Notice(_) => item.text().map(Cow::into_owned),
        _ => None,
    })
}

pub(crate) fn select(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), choices(values))
}

pub(crate) fn choices(values: &[(&str, &str)]) -> Vec<acp::SessionConfigSelectOption> {
    values
        .iter()
        .map(|&(value, label)| acp::SessionConfigSelectOption::new(value.to_string(), label.to_string()))
        .collect()
}

pub(crate) fn model_picker(
    current: &str,
    choices: impl Into<acp::SessionConfigSelectOptions>,
) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select("model", "Model", current.to_string(), choices)
        .meta(ConfigOptionMeta { multi_select: true }.into_meta())
}

pub(crate) fn model_choice(value: &str, name: &str, meta: SelectOptionMeta) -> acp::SessionConfigSelectOption {
    acp::SessionConfigSelectOption::new(value.to_string(), name.to_string()).meta(meta.into_meta())
}

pub(crate) fn option_group(name: &str, options: Vec<acp::SessionConfigSelectOption>) -> acp::SessionConfigSelectGroup {
    acp::SessionConfigSelectGroup::new(name.to_string(), name.to_string(), options)
}

pub(crate) fn sonnet_model() -> acp::SessionConfigOption {
    select("model", "Model", "claude-sonnet", &[("claude-sonnet", "Claude Sonnet")])
}

pub(crate) fn status_ui() -> TestUi {
    TestUiBuilder::new().dimensions(120, 15).config_options(vec![sonnet_model()]).build()
}

pub(crate) fn config_update(options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options)))
}

pub(crate) fn mcp_notification(servers: Vec<McpServerStatusEntry>) -> AcpEvent {
    AcpEvent::McpNotification(McpNotification::ServerStatus { servers })
}

pub(crate) fn select_option(id: &str, current_value: &str) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(id.to_string(), id.to_string(), current_value.to_string(), plain(&[current_value]))
}

pub(crate) fn reasoning_option(current: &str, levels: &[&str]) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select("reasoning_effort", "Reasoning", current.to_string(), plain(levels))
}

pub(crate) fn mode_option(current: &str, modes: &[&str]) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), plain(modes))
        .category(acp::SessionConfigOptionCategory::Mode)
}

/// A mode option whose values arrive under group headers rather than as a flat list.
pub(crate) fn grouped_mode_option(current: &str, groups: &[(&str, &[&str])]) -> acp::SessionConfigOption {
    let groups: Vec<_> = groups.iter().map(|(name, modes)| option_group(name, plain(modes))).collect();
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), groups)
        .category(acp::SessionConfigOptionCategory::Mode)
}

pub(crate) fn open_file_picker(composer: &mut Composer, files: &[&str]) {
    let FilesystemCommand::IndexFiles { request_id, root } = composer.open_file_picker(Path::new("/workspace")) else {
        panic!("opening the file picker should ask for a file index");
    };
    let mut filesystem = FakeFilesystem::new();
    for file in files {
        filesystem.write_file(root.join(file), "");
    }
    composer.on_files_indexed(request_id, filesystem.index_files(&root));
}

pub(crate) fn assert_shell_command_highlighted(ui: &mut TestUi, tool: &str, command: &str) {
    let conversation = ui.conversation();
    let row = row_containing(&conversation, command).expect("rendered shell command");
    let text = row_text(&conversation, row);
    assert!(text.contains(&format!("{tool} {command}")), "command should share the tool row: {text:?}");
    let start = text[..text.find(command).expect("command position")].width();
    let cells = row_cells(&conversation, row);
    assert_eq!(cells[start - 1].bg, Color::Reset, "the gap before the command should not set a background");
    let command_cells: Vec<_> =
        cells[start..start + command.width()].iter().filter(|cell| cell.symbol() != " ").collect();
    assert!(command_cells.iter().all(|cell| cell.bg == Color::Reset), "command should sit on the terminal background");
    let keyword = command_cells.iter().find(|cell| cell.symbol() == "i").expect("if keyword");
    let variable = command_cells.iter().find(|cell| cell.symbol() == "$").expect("shell variable");
    assert_ne!(keyword.fg, variable.fg, "shell keywords and variables should use distinct token colors");
}

pub(crate) fn plan_meta(markdown: &str) -> ArtifactReviewElicitationMeta {
    ArtifactReviewElicitationMeta::new(Some(PathBuf::from("/tmp/plan.md")), "Review /tmp/plan.md", markdown)
}

/// Opens `/settings` and then the menu row labelled `label`, wherever it sits.
pub(crate) fn open_settings_row(ui: &mut TestUi, label: &str) {
    ui.run_command("/settings");
    ui.click_text(label);
}

pub(crate) fn open_workspace_picker(ui: &mut TestUi, workspaces: Vec<WorkspaceEntry>) {
    ui.run_command("/move");
    let command = ui.next_agent_command();
    assert!(
        matches!(command, Some(AgentCommand::ListWorkspaces { .. })),
        "/move should list workspaces, got {command:?}"
    );
    ui.deliver_result(workspaces_listed(workspaces));
}

pub(crate) fn move_to_other_workspace(ui: &mut TestUi) {
    open_workspace_picker(
        ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/other", false)],
    );
    ui.key(key(KeyCode::Enter));
    let command = ui.next_agent_command();
    assert!(
        matches!(
            &command,
            Some(AgentCommand::MoveWorkspace { session_id, target: WorkspaceMoveTarget::Existing { path } })
                if session_id == "test-session" && path == std::path::Path::new("/home/user/code/other")
        ),
        "picking a workspace should move the session there, got {command:?}"
    );
}

pub(crate) fn assert_sets_config_option(ui: &mut TestUi, id: &str, value: &str) {
    let command = ui.next_agent_command();
    assert!(
        matches!(&command, Some(AgentCommand::SetConfigOption { config_id, value: set, .. })
            if config_id == id && *set == acp::SessionConfigOptionValue::id(value)),
        "expected {id} to be set to {value}, got {command:?}"
    );
}

pub(crate) fn assert_authenticates_mcp_server(ui: &mut TestUi, name: &str) {
    let command = ui.next_agent_command();
    assert!(
        matches!(&command, Some(AgentCommand::AuthenticateMcpServer { server_name, .. }) if server_name == name),
        "expected to authenticate {name}, got {command:?}"
    );
}

pub(crate) fn take_applied_theme(ui: &mut TestUi) -> Box<UiSettings> {
    ui.take_command_where(|command| match command {
        Command::Host(HostCommand::ApplyTheme { settings }) => Some(settings),
        _ => None,
    })
    .expect("the app should ask to apply a theme")
}

pub(crate) fn bells(ui: &mut TestUi) -> usize {
    ui.count_commands(|command| matches!(command, Command::Host(HostCommand::RingBell)))
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
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(id.to_string()).title(title)))
}

pub(crate) fn context_usage(used: u64, limit: u64) -> AcpEvent {
    session_update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(used, limit)))
}

pub(crate) fn tool_completed_with_diff(id: &str) -> AcpEvent {
    tool_completed_with_diff_contents(id, "fn old_name() {}\n", "fn new_name() {}\n")
}

pub(crate) fn tool_completed_with_diff_contents(id: &str, old: &str, new: &str) -> AcpEvent {
    let diff = text_diff("/src/main.rs", old, new);
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string())
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Completed),
    ))
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

pub(crate) fn session_loaded(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> CommandResult {
    CommandResult::ResumeSession {
        session_id: SessionId::new(session_id),
        result: Ok(acp::ResumeSessionResponse::default().config_options(config_options)),
    }
}

pub(crate) fn new_session_created(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> CommandResult {
    CommandResult::NewSession(Ok(
        acp::NewSessionResponse::new(SessionId::new(session_id)).config_options(config_options)
    ))
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
    acp::UpdateSessionNotification::new(SessionId::new(session_id), update).into()
}

pub(crate) fn user_message_chunk(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::UserMessage(
        acp::UserMessage::new("user-message").content(vec![acp::ContentBlock::Text(acp::TextContent::new(text))]),
    )
}

pub(crate) fn workspace_entry(path: &str, is_current: bool) -> WorkspaceEntry {
    WorkspaceEntry { path: std::path::PathBuf::from(path), is_current }
}

pub(crate) fn workspaces_listed(workspaces: Vec<WorkspaceEntry>) -> CommandResult {
    CommandResult::WorkspacesListed(Ok(WorkspaceListResponse { workspaces }))
}

pub(crate) fn workspace_list_failed(error: &str) -> CommandResult {
    CommandResult::WorkspacesListed(Err(error.to_string()))
}

pub(crate) fn workspace_moved(new_cwd: &str) -> CommandResult {
    CommandResult::WorkspaceMoved(Ok(WorkspaceMoveResponse { new_cwd: std::path::PathBuf::from(new_cwd) }))
}

pub(crate) fn workspace_move_failed(error: &str) -> CommandResult {
    CommandResult::WorkspaceMoved(Err(error.to_string()))
}

pub(crate) fn session_load_failed(error: &str) -> CommandResult {
    CommandResult::ResumeSession { session_id: "test-session".into(), result: Err(error.to_string()) }
}

pub(crate) fn search(ui: &mut TestUi, query: &str, results: Vec<PromptSearchResult>) {
    start_search(ui, query);
    ui.deliver_result(search_results(query, results));
}

pub(crate) fn start_search(ui: &mut TestUi, query: &str) {
    ui.key(ctrl('r'));
    ui.type_text(query);
    let searched = std::iter::from_fn(|| ui.next_agent_command())
        .any(|command| matches!(command, AgentCommand::SearchPrompts(params) if params.query == query));
    assert!(searched, "typing {query:?} should search prompt history");
}

pub(crate) fn search_results(query: &str, results: Vec<PromptSearchResult>) -> CommandResult {
    CommandResult::PromptSearchResults {
        query: query.to_string(),
        result: Ok(PromptSearchResponse { query: query.to_string(), results, truncated: false }),
    }
}

pub(crate) fn hit(prompt: &str, match_start: usize, match_end: usize) -> PromptSearchResult {
    PromptSearchResult {
        session_id: "s1".to_string(),
        cwd: "/tmp/repo".into(),
        session_created_at: "2026-05-17T00:00:00Z".to_string(),
        prompt: prompt.to_string(),
        match_start,
        match_end,
    }
}

fn plain(values: &[&str]) -> Vec<acp::SessionConfigSelectOption> {
    values.iter().map(|&value| acp::SessionConfigSelectOption::new(value.to_string(), value.to_string())).collect()
}
