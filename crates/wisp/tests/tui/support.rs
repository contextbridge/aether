pub(crate) use acp_utils::client::AcpEvent;
pub(crate) use acp_utils::config_meta::SelectOptionMeta;
pub(crate) use acp_utils::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, McpNotification, McpRequest, McpServerAuthCapability,
    McpServerStatus, McpServerStatusEntry, PromptSearchParams, PromptSearchResult, SessionPreviewParams,
    WorkspaceEntry, WorkspaceListParams, WorkspaceListResponse, WorkspaceMoveParams, WorkspaceMoveResponse,
    WorkspaceMoveTarget,
};
pub(crate) use acp_utils::testing::{
    FakeAgent, announced, choices, idle_notification, model_choice, model_picker, on_session, option_group, plan,
    prompt_search_hit, prompt_search_response, running_state, select, session_preview, user_message,
};
pub(crate) use agent_client_protocol::Error;
pub(crate) use agent_client_protocol::schema::v2::{
    self as acp, BooleanPropertySchema, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ElicitationUrlMode, SessionId,
    StringPropertySchema,
};
pub(crate) use client_core::composer::attachment::PromptAttachment;
pub(crate) use client_core::session_core::CommandEntry;

pub(crate) use client_core::session_core::{ForegroundOperation, SessionCore};
pub(crate) use client_core::testing::{FakeGit, SessionCoreConfigBuilder, elicitation_id};
pub(crate) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use ratatui::buffer::{Buffer, Cell};
pub(crate) use ratatui::layout::Position;
pub(crate) use ratatui::style::{Color, Modifier};
use std::borrow::Cow;
pub(crate) use std::fmt::Write as FmtWrite;
pub(crate) use std::io::Write as IoWrite;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::time::{Duration, Instant};
pub(crate) use unicode_width::UnicodeWidthStr;
pub(crate) use utils::artifact_review::ArtifactReviewElicitationMeta;
pub(crate) use utils::testing::TestWorkspace;
pub(crate) use wisp::command::HostCommandResult;
pub(crate) use wisp::testing::ElicitationReply;
pub(crate) use wisp::testing::{
    BackendEvent, InstrumentedBackend, StreamContent, TestUi, TestUiBuilder, assert_buffer_eq, buffer_text,
    chunk_message, compaction_update, context_cleared, git_diff_screen, has_cell, line_text, numbered_lines,
    render_surface, row_cells, row_containing, row_text, rows_with_background, session_update, sub_agent_finished,
    sub_agent_started, sub_agent_tool_update, text_chunk, text_chunk_with_id, text_diff, thought_chunk,
    thought_chunk_with_id, tool_completed, tool_status,
};

pub(crate) use acp_utils::conversation::{ConversationContent, ConversationItem, ItemState, ToolCall, ToolStatus};
pub(crate) use clankerdiff_ratatui::diff::{FileStatus, StageState};
pub(crate) use wisp::settings::overlay::SettingsOverlay;
pub(crate) use wisp::settings::{StatusLineSegmentConfig, UiSettings};
pub(crate) use wisp::surfaces::composer::Composer;
pub(crate) use wisp::surfaces::picker::SlashCommand;
pub(crate) use wisp::theme::Theme;
pub(crate) use wisp::view::input::UiEvent;
pub(crate) use wisp::view::syntax::SyntaxHighlighter;

pub(crate) type Step = fn(&mut TestUi);

pub(crate) type ListSessions = acp::ListSessionsRequest;
pub(crate) type Resume = acp::ResumeSessionRequest;
pub(crate) type NewSession = acp::NewSessionRequest;
pub(crate) type Login = acp::LoginAuthRequest;
pub(crate) type Preview = SessionPreviewParams;
pub(crate) type Search = PromptSearchParams;
pub(crate) type ListWorkspaces = WorkspaceListParams;
pub(crate) type MoveWorkspace = WorkspaceMoveParams;

pub(crate) fn sent_prompt(ui: &mut TestUi) -> acp::PromptRequest {
    ui.agent().prompt.try_recv().expect("the agent received a prompt")
}

pub(crate) fn sent_prompts(ui: &mut TestUi) -> Vec<String> {
    std::iter::from_fn(|| ui.agent().prompt.try_recv().ok())
        .map(|request| prompt_text(&request).unwrap_or_default().to_owned())
        .collect()
}

pub(crate) fn sent_any_prompt(ui: &mut TestUi) -> bool {
    ui.agent().prompt.try_recv().is_ok()
}

pub(crate) fn prompt_text(request: &acp::PromptRequest) -> Option<&str> {
    match request.prompt.first()? {
        acp::ContentBlock::Text(text) => Some(&text.text),
        _ => None,
    }
}

pub(crate) fn agent_error(message: &str) -> Error {
    Error::new(-32603, message)
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
    app.app().core().conversation().items().iter().filter_map(|item| match item.content() {
        ConversationContent::User(_) => item.text().map(Cow::into_owned),
        ConversationContent::Notice(notice) => Some(notice.text.clone()),
        _ => None,
    })
}

pub(crate) fn sonnet_model() -> acp::SessionConfigOption {
    select("model", "Model", "claude-sonnet", &[("claude-sonnet", "Claude Sonnet")])
}

pub(crate) fn status_ui() -> TestUi {
    TestUiBuilder::new().session(|session| session.config_options(vec![sonnet_model()])).dimensions(120, 15).build()
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
    let root = Path::new("/workspace");
    composer.open_file_picker();
    let mut entries: Vec<_> = files.iter().map(|file| PromptAttachment::new(root.join(file), root)).collect();
    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    composer.on_files_indexed(entries);
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
    ui.answer::<ListWorkspaces>(Ok(WorkspaceListResponse { workspaces }));
}

pub(crate) fn move_to_other_workspace(ui: &mut TestUi) {
    open_workspace_picker(
        ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/other", false)],
    );
    ui.key(key(KeyCode::Enter));
}

pub(crate) fn workspace_moved(ui: &mut TestUi, new_cwd: &str) -> WorkspaceMoveParams {
    ui.answer::<MoveWorkspace>(Ok(WorkspaceMoveResponse { new_cwd: PathBuf::from(new_cwd) }))
}

pub(crate) fn session_loaded(
    ui: &mut TestUi,
    config_options: Vec<acp::SessionConfigOption>,
) -> acp::ResumeSessionRequest {
    ui.answer::<Resume>(Ok(acp::ResumeSessionResponse::new().config_options(config_options)))
}

pub(crate) fn new_session_created(ui: &mut TestUi, session_id: &str, config_options: Vec<acp::SessionConfigOption>) {
    ui.answer::<NewSession>(Ok(acp::NewSessionResponse::new(session_id.to_string()).config_options(config_options)));
}

pub(crate) fn assert_sets_config_option(ui: &mut TestUi, id: &str, value: &str) {
    let request = ui.agent().config.try_recv().expect("the agent was asked to change a setting");
    assert_eq!(request.config_id, acp::SessionConfigId::new(id.to_string()));
    assert_eq!(request.value, acp::SessionConfigOptionValue::id(value));
}

pub(crate) fn assert_authenticates_mcp_server(ui: &mut TestUi, name: &str) {
    let request = ui.agent().mcp.try_recv().expect("the agent was asked to authenticate a server");
    assert!(matches!(&request, McpRequest::Authenticate { server_name, .. } if server_name == name), "{request:?}");
}

pub(crate) fn take_applied_theme(ui: &mut TestUi) -> UiSettings {
    ui.applied_themes().into_iter().next().expect("the app should ask to apply a theme")
}

pub(crate) fn assert_ctrl_c_exits(ui: &mut TestUi) {
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

pub(crate) fn workspace_entry(path: &str, is_current: bool) -> WorkspaceEntry {
    WorkspaceEntry { path: std::path::PathBuf::from(path), is_current }
}

pub(crate) fn search(ui: &mut TestUi, query: &str, results: Vec<PromptSearchResult>) {
    start_search(ui, query);
    answer_search(ui, query, Ok(prompt_search_response(query, results)));
}

pub(crate) fn start_search(ui: &mut TestUi, query: &str) {
    ui.key(ctrl('r'));
    ui.type_text(query);
}

pub(crate) fn answer_search(
    ui: &mut TestUi,
    query: &str,
    response: Result<acp_utils::notifications::PromptSearchResponse, Error>,
) {
    loop {
        let (request, responder) = ui.held::<Search>();
        if request.query == query {
            return ui.respond(responder, response);
        }
        let _ = responder.respond(prompt_search_response(&request.query, Vec::new()));
    }
}

fn plain(values: &[&str]) -> Vec<acp::SessionConfigSelectOption> {
    values.iter().map(|&value| acp::SessionConfigSelectOption::new(value.to_string(), value.to_string())).collect()
}
