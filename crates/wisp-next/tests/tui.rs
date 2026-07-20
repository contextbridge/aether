use acp_utils::ElicitationSchema;
use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::notifications::{
    ContextClearedParams, ContextUsage, ContextUsageParams, CreateElicitationRequestParams, ElicitationAction,
    ElicitationParams,
};
use acp_utils::testing::test_connection;
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::LocalSet;
use wisp_next::app::{App, AppConfig, HistoryItem};
use wisp_next::composer::Composer;
use wisp_next::picker::CommandEntry;
use wisp_next::presentation::TranscriptRenderer;
use wisp_next::render::sync_terminal as sync_terminal_with_renderer;
use wisp_next::settings::UiSettings;
use wisp_next::theme::Theme;
use wisp_next::{inline_viewport_height, workspace_status::WorkspaceStatus};

fn make_app() -> (App, UnboundedReceiver<PromptCommand>) {
    make_app_in(std::path::PathBuf::from("."))
}

fn make_app_in(working_dir: std::path::PathBuf) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir,
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

#[test]
fn composer_soft_wraps_and_tracks_cursor() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefgh");
    composer.move_left();
    composer.move_left();

    let layout = composer.layout(6, &Theme::default());

    assert_eq!(layout.lines.len(), 2);
    assert_eq!(layout.cursor.x, 4);
    assert_eq!(layout.cursor.y, 1);
}

#[test]
fn composer_moves_vertically_before_recalling_history() {
    let mut composer = Composer::new();
    composer.insert_str("one\nsecond");
    composer.move_left();
    composer.move_left();

    assert!(composer.move_up());
    assert_eq!(composer.cursor_position(), (0, 3));
    assert!(composer.move_down());
    assert_eq!(composer.cursor_position(), (1, 3));
}

#[test]
fn command_picker_filters_and_applies_selected_command() {
    let mut composer = Composer::new();
    composer.insert_char('/');
    composer.open_command_picker(vec![
        CommandEntry {
            name: "search".to_string(),
            description: "Search the workspace".to_string(),
            has_input: true,
            hint: Some("query".to_string()),
        },
        CommandEntry {
            name: "status".to_string(),
            description: "Show status".to_string(),
            has_input: false,
            hint: None,
        },
    ]);
    composer.insert_str("sea");
    composer.refresh_overlay_query();

    assert_eq!(composer.overlay_query(), Some("sea"));
    assert!(composer.overlay_lines(80, 6, &Theme::default()).iter().any(|line| line_text(line).contains("/search")));

    let selected = composer.accept_command().unwrap();
    assert_eq!(selected.name, "search");
    assert_eq!(composer.text(), "/search");
    assert!(!composer.has_overlay());
}

#[test]
fn file_picker_filters_and_inserts_a_mention() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("src")).unwrap();
    std::fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(directory.path().join("README.md"), "# demo\n").unwrap();

    let mut composer = Composer::new();
    composer.insert_char('@');
    composer.open_file_picker(directory.path());
    composer.insert_str("main");
    composer.refresh_overlay_query();

    assert_eq!(composer.overlay_query(), Some("main"));
    assert!(
        composer.overlay_lines(80, 6, &Theme::default()).iter().any(|line| line_text(line).contains("src/main.rs"))
    );

    let selected = composer.accept_file().unwrap();
    assert_eq!(selected.display_name, "src/main.rs");
    assert_eq!(composer.text(), "@src/main.rs ");
    assert!(!composer.has_overlay());
}

#[test]
fn selected_file_is_sent_as_an_acp_resource_attachment() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("context.txt"), "attached context").unwrap();
    let (mut app, mut command_rx) = make_app_in(directory.path().to_path_buf());

    app.on_key(key(KeyCode::Char('@')));
    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.composer().text(), "@context.txt ");
    app.on_key(key(KeyCode::Enter));

    let PromptCommand::Prompt { text, content, .. } = command_rx.try_recv().unwrap() else {
        panic!("expected a prompt command");
    };
    assert_eq!(text, "@context.txt ");
    assert!(matches!(content.as_deref(), Some([acp::ContentBlock::Resource(_)])));
}

#[test]
fn file_picker_renders_in_the_live_viewport_not_scrollback() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("context.txt"), "attached context").unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(key(KeyCode::Char('@')));
    app.on_key(key(KeyCode::Char('c')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(buffer_text(terminal.backend().buffer()).contains("context.txt"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("context.txt"));
}

#[test]
fn composer_history_restores_the_unsubmitted_draft() {
    let mut composer = Composer::new();
    composer.insert_str("first");
    assert_eq!(composer.take_submission(), "first");
    composer.insert_str("draft");

    assert!(composer.recall_previous());
    assert_eq!(composer.text(), "first");
    assert!(composer.recall_next());
    assert_eq!(composer.text(), "draft");
}

#[test]
fn markdown_styles_stream_live_and_finalize_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let heading = renderer.theme().heading;
    submit_prompt(&mut app, "render markdown");
    app.on_acp_event(text_chunk("# Heading\n\n**boldword** and *italicword*"));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    assert!(buffer_text(&conversation).contains("# Heading"));
    assert!(has_cell(&conversation, "H", |cell| cell.fg == heading && cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "b", |cell| cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "i", |cell| cell.modifier.contains(Modifier::ITALIC)));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert_eq!(conversation.matches("Heading").count(), 1);
    assert!(!conversation.contains("**boldword**"));
}

#[test]
fn fenced_code_is_syntax_highlighted_with_code_background() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let code_background = renderer.theme().code_bg;
    submit_prompt(&mut app, "show code");
    app.on_acp_event(text_chunk("```rust\nfn highlighted() {}\n```"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    assert!(buffer_text(&conversation).contains("fn highlighted()"));
    assert!(has_cell(&conversation, "f", |cell| cell.bg == code_background));
}

#[test]
fn completed_tool_diff_is_themed_and_rendered_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let removed_background = renderer.theme().diff_removed_bg;
    let added_background = renderer.theme().diff_added_bg;
    submit_prompt(&mut app, "edit file");
    app.on_acp_event(tool_call("edit-1", "Edit src/main.rs"));
    app.on_acp_event(tool_completed_with_diff("edit-1"));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    let text = buffer_text(&conversation);
    assert_eq!(text.matches("old_name").count(), 1);
    assert_eq!(text.matches("new_name").count(), 1);
    assert!(has_cell(&conversation, "-", |cell| cell.bg == removed_background));
    assert!(has_cell(&conversation, "+", |cell| cell.bg == added_background));
}

#[test]
fn wide_diff_uses_side_by_side_layout() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(100);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "edit file");
    app.on_acp_event(tool_call("edit-1", "Edit src/main.rs"));
    app.on_acp_event(tool_completed_with_diff("edit-1"));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let text = buffer_text(&conversation_buffer(&mut terminal));
    assert!(text.lines().any(|line| line.contains("old_name") && line.contains("new_name")), "{text}");
}

#[test]
fn wide_diff_marks_truncated_panel_content() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(100);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "edit file");
    app.on_acp_event(tool_call("edit-1", "Edit src/main.rs"));
    app.on_acp_event(tool_completed_with_diff_contents(
        "edit-1",
        &format!("old_{}\n", "x".repeat(80)),
        &format!("new_{}\n", "y".repeat(80)),
    ));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let text = buffer_text(&conversation_buffer(&mut terminal));
    assert!(text.contains('…'), "expected visibly truncated split diff:\n{text}");
}

#[test]
fn markdown_blockquote_prefixes_inline_code_first_content() {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let lines = renderer.history_lines(&[HistoryItem::Text("> `quoted`".to_string())], None, 40, 0, 0);

    assert_eq!(line_text(&lines[0]), "  quoted");
}

#[test]
fn markdown_horizontal_rule_uses_available_width() {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let lines = renderer.history_lines(&[HistoryItem::Text("---".to_string())], None, 20, 0, 0);

    assert_eq!(line_text(&lines[0]), "─".repeat(20));
}

#[test]
fn transcript_wrapping_expands_tabs() {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let lines = renderer.history_lines(&[HistoryItem::Text("a\tb".to_string())], None, 4, 0, 0);

    assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>(), ["a   ", "b"]);
}

#[test]
fn transcript_wrapping_never_exceeds_a_one_column_allocation() {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let lines = renderer.history_lines(&[HistoryItem::Text("界".to_string())], None, 1, 0, 0);

    assert!(lines.iter().all(|line| line.width() <= 1), "{lines:?}");
    assert_eq!(line_text(&lines[0]), "…");
}

#[test]
fn trailing_newline_does_not_add_an_empty_user_content_row() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let user_background = renderer.theme().sidebar_bg;
    type_text(&mut app, "hello");
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.on_key(key(KeyCode::Enter));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let styled_rows = rows_with_background(&conversation_buffer(&mut terminal), user_background);
    assert_eq!(styled_rows, 3);
}

#[test]
fn markdown_renders_lists_strikethrough_and_tables() {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let markdown = "- first\n- second\n\n~~removed~~\n\n| Name | Value |\n| --- | --- |\n| alpha | beta |";

    let lines = renderer.history_lines(&[HistoryItem::Text(markdown.to_string())], None, 40, 0, 0);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(text.contains("• first"), "{text}");
    assert!(text.contains("• second"), "{text}");
    assert!(text.contains("Name | Value"), "{text}");
    assert!(text.contains("alpha | beta"), "{text}");
    assert!(
        lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content.contains("removed") && span.style.add_modifier.contains(Modifier::CROSSED_OUT)
        })
    );
}

#[test]
fn theme_loads_semantic_colors_from_tmtheme_file() {
    let mut file = tempfile::Builder::new().suffix(".tmTheme").tempfile().unwrap();
    write!(
        file,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>name</key><string>Test</string><key>settings</key><array>
<dict><key>settings</key><dict><key>foreground</key><string>#112233</string><key>background</key><string>#010203</string><key>caret</key><string>#445566</string></dict></dict>
<dict><key>scope</key><string>markup.heading</string><key>settings</key><dict><key>foreground</key><string>#abcdef</string></dict></dict>
</array></dict></plist>"#
    )
    .unwrap();

    let theme = Theme::load_from_path(file.path());

    assert_eq!(theme.text_primary, Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(theme.background, Color::Rgb(0x01, 0x02, 0x03));
    assert_eq!(theme.accent, Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(theme.heading, Color::Rgb(0xab, 0xcd, 0xef));
}

#[test]
fn large_markdown_history_preserves_order_across_scrollback_and_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let mut markdown = String::new();
    for index in 0..40 {
        writeln!(markdown, "paragraph-{index}\n").unwrap();
    }
    submit_prompt(&mut app, "long response");
    app.on_acp_event(text_chunk(&markdown));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    let text = buffer_text(&conversation);
    assert_eq!(text.matches("paragraph-0").count(), 1, "conversation:\n{text}");
    assert_eq!(text.matches("paragraph-39").count(), 1);
    assert!(text.find("paragraph-0").unwrap() < text.find("paragraph-39").unwrap());
}

#[test]
fn settings_ignore_unknown_legacy_fields() {
    let settings: UiSettings = serde_json::from_str(
        r#"{"contentPadding":4,"theme":{"file":"nord.tmTheme","future":true},"statusLine":{"left":[]}}"#,
    )
    .unwrap();

    assert_eq!(settings.content_padding, Some(4));
    assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
}

#[test]
fn inline_viewport_reserves_two_rows_for_scrollback() {
    assert_eq!(inline_viewport_height(15), 13);
    assert_eq!(inline_viewport_height(3), 1);
    assert_eq!(inline_viewport_height(2), 1);
}

#[test]
fn one_row_inline_viewport_draws_without_panicking() {
    let (mut app, _command_rx) = make_app();
    let terminal_height = 3;
    let mut terminal = Terminal::with_options(
        TestBackend::new(40, terminal_height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(terminal_height)) },
    )
    .unwrap();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert_eq!(terminal.get_frame().area().height, 1);
}

#[test]
fn live_viewport_is_drawn_before_history_is_inserted() {
    let (mut app, _command_rx) = make_app();
    let backend = RecordingBackend::new(40, 15);
    let mut terminal =
        Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) })
            .unwrap();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    terminal.backend_mut().events.clear();

    let mut response = String::new();
    for index in 0..20 {
        writeln!(response, "line-{index}\n").unwrap();
    }
    submit_prompt(&mut app, "hello");
    app.on_acp_event(text_chunk(&response));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let events = &terminal.backend().events;
    let draw = events.iter().position(|event| *event == BackendEvent::ShowCursor).unwrap();
    let insert = events.iter().position(|event| matches!(event, BackendEvent::Scroll)).unwrap();
    assert!(draw < insert, "expected viewport draw before history insertion: {events:?}");
}

#[test]
fn status_line_is_never_inserted_into_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = Terminal::with_options(
        TestBackend::new(40, 15),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) },
    )
    .unwrap();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    submit_prompt(&mut app, "hello");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let scrollback = buffer_text(&history_buffer(&mut terminal));
    assert!(!scrollback.contains("working"), "{scrollback}");
    assert!(!scrollback.contains("esc to cancel"), "{scrollback}");
    assert!(!scrollback.contains("aether"), "{scrollback}");
}

#[test]
fn history_waits_until_resized_viewport_has_scrollback_room() {
    let (mut app, _command_rx) = make_app();
    let terminal_height = 15;
    let mut terminal = Terminal::with_options(
        TestBackend::new(40, terminal_height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(terminal_height)) },
    )
    .unwrap();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    terminal.backend_mut().resize(40, 10);
    submit_prompt(&mut app, "queued while small");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(!buffer_text(terminal.backend().scrollback()).contains("queued while small"));
    assert!(buffer_text(terminal.backend().buffer()).contains("queued while small"));

    terminal.backend_mut().resize(40, terminal_height);
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(app.pending_items().is_empty());
    assert!(buffer_text(terminal.backend().buffer()).contains("queued while small"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendEvent {
    ShowCursor,
    Scroll,
}

#[derive(Debug)]
struct RecordingBackend {
    inner: TestBackend,
    events: Vec<BackendEvent>,
}

impl RecordingBackend {
    fn new(width: u16, height: u16) -> Self {
        Self { inner: TestBackend::new(width, height), events: Vec::new() }
    }
}

impl Backend for RecordingBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, lines: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(lines)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::ShowCursor);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        self.inner.scroll_region_up(region, lines)
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        self.inner.scroll_region_down(region, lines)
    }
}

fn make_terminal() -> Terminal<TestBackend> {
    make_terminal_with_width(40)
}

fn make_terminal_with_width(width: u16) -> Terminal<TestBackend> {
    let terminal_height = 15;
    Terminal::with_options(
        TestBackend::new(width, terminal_height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(terminal_height)) },
    )
    .unwrap()
}

fn sync_terminal(terminal: &mut Terminal<TestBackend>, app: &mut App) -> Result<(), std::convert::Infallible> {
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(terminal, app, &mut renderer)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn submit_prompt(app: &mut App, text: &str) {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
}

fn session_update(update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new("test-session"), update: Box::new(update) }
}

fn text_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

fn tool_call(id: &str, title: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title)))
}

fn tool_completed(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn tool_completed_with_diff(id: &str) -> AcpEvent {
    tool_completed_with_diff_contents(id, "fn old_name() {}\n", "fn new_name() {}\n")
}

fn tool_completed_with_diff_contents(id: &str, old: &str, new: &str) -> AcpEvent {
    let diff = acp::Diff::new("src/main.rs", new).old_text(old);
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Completed),
    )))
}

fn viewport_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
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

fn history_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
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

fn conversation_buffer(terminal: &mut Terminal<TestBackend>) -> Buffer {
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

fn has_cell(buffer: &Buffer, symbol: &str, predicate: impl Fn(&Cell) -> bool) -> bool {
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

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

fn rows_with_background(buffer: &Buffer, background: ratatui::style::Color) -> usize {
    (buffer.area.top()..buffer.area.bottom())
        .filter(|&y| {
            (buffer.area.left()..buffer.area.right())
                .any(|x| buffer.cell((x, y)).is_some_and(|cell| cell.bg == background))
        })
        .count()
}

#[test]
fn submit_sends_prompt_and_clears_composer() {
    let (mut app, mut command_rx) = make_app();

    submit_prompt(&mut app, "hello agent");

    match command_rx.try_recv().unwrap() {
        PromptCommand::Prompt { session_id, text, content } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(text, "hello agent");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
    assert!(app.composer().is_empty());
}

#[test]
fn submit_is_ignored_while_prompt_in_flight() {
    let (mut app, mut command_rx) = make_app();
    submit_prompt(&mut app, "first");
    command_rx.try_recv().unwrap();

    submit_prompt(&mut app, "second");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "second");
}

#[test]
fn esc_cancels_only_while_busy() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(key(KeyCode::Esc));
    assert!(command_rx.try_recv().is_err());

    submit_prompt(&mut app, "work");
    command_rx.try_recv().unwrap();
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(command_rx.try_recv().unwrap(), PromptCommand::Cancel { .. }));
}

#[test]
fn double_ctrl_c_exits_and_first_press_clears_composer() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "draft");

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
    assert!(app.composer().is_empty());
    assert!(app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(app.exit_requested());
}

#[test]
fn ctrl_c_confirmation_disarms_after_window() {
    let (mut app, _command_rx) = make_app();

    app.on_key(ctrl('c'));
    app.on_tick(Instant::now() + Duration::from_secs(2));
    assert!(!app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
}

#[test]
fn connection_closed_requests_exit() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ConnectionClosed);

    assert!(app.exit_requested());
}

#[test]
fn alt_enter_inserts_newline_instead_of_submitting() {
    let (mut app, mut command_rx) = make_app();
    type_text(&mut app, "line one");

    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_text(&mut app, "line two");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "line one\nline two");
}

#[test]
fn context_usage_is_reported_as_percent() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ContextUsage(ContextUsageParams {
        usage: ContextUsage { context_limit: Some(200_000), input_tokens: 50_000, ..ContextUsage::default() },
    }));

    assert_eq!(app.context_percent(), Some(25));
}

#[test]
fn context_clear_discards_conversation_retained_in_the_live_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "old retained message");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("old retained message"));

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("old retained message"), "{viewport}");
    assert!(viewport.contains("[wisp-next] Context cleared"), "{viewport}");
}

#[test]
fn fitting_user_message_remains_in_the_live_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    submit_prompt(&mut app, "hello viewport");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("hello viewport"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("hello viewport"));
}

#[test]
fn completed_stream_lines_remain_live_until_they_overflow() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "hi");

    let mut completed = String::new();
    for index in 0..20 {
        writeln!(completed, "line-{index}\n").unwrap();
    }
    app.on_acp_event(text_chunk(&format!("{completed}partial")));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let scrollback = buffer_text(&history_buffer(&mut terminal));
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(scrollback.contains("line-0"));
    assert!(!scrollback.contains("partial"));
    assert!(viewport.contains("line-19"));
    assert!(viewport.contains("partial"));
}

#[test]
fn completed_streaming_text_remains_adjacent_to_the_composer_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "hi");

    app.on_acp_event(text_chunk("streamed "));
    app.on_acp_event(text_chunk("answer"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("streamed answer"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("streamed answer"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert_eq!(conversation.matches("streamed answer").count(), 1);
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("streamed answer"));
}

#[test]
fn running_tool_holds_later_content_out_of_committed_history() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "run a tool");

    app.on_acp_event(tool_call("tool-1", "Reading main.rs"));
    app.on_acp_event(text_chunk("tool output summary"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Reading main.rs"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("Reading main.rs"));

    app.on_acp_event(tool_completed("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(conversation.contains("Reading main.rs"));
    assert!(conversation.contains("tool output summary"));
    assert!(app.pending_items().is_empty());
}

#[test]
fn cancelled_prompt_marks_running_tool_as_error() {
    let (mut app, _command_rx) = make_app();
    submit_prompt(&mut app, "run a tool");
    app.on_acp_event(tool_call("tool-1", "Slow tool"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    let items = app.drain_finalized();
    let cancelled = items.iter().any(|item| {
        matches!(item, wisp_next::app::HistoryItem::Tool { status: wisp_next::tool_calls::ToolStatus::Error(cause), .. } if cause == "cancelled")
    });
    assert!(cancelled, "expected cancelled tool in {items:?}");
}

#[test]
fn short_streaming_message_renders_directly_above_composer() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "prompt");
    app.on_acp_event(text_chunk("short answer"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = viewport_buffer(&mut terminal);
    let prompt_row = row_containing(&viewport, "prompt").unwrap();
    let message_row = row_containing(&viewport, "short answer").unwrap();
    let composer_row = row_containing(&viewport, "> ").unwrap();
    assert!(prompt_row < message_row);
    assert_eq!(message_row + 1, composer_row);
}

#[test]
fn composer_echo_and_status_line_render_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    type_text(&mut app, "typing");
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("> typing"));
    assert!(viewport.contains("~/code/demo · main"));
    assert!(viewport.contains("aether"));
}

async fn settle_screen_effects(app: &mut App) {
    while let Some(effect) = app.take_screen_effect() {
        app.on_screen_event(effect.execute().await);
    }
}

fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn init_git_repo(dir: &std::path::Path) {
    run_git(dir, &["init", "--quiet"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn ctrl_g_opens_and_esc_closes_git_diff() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal();

    app.on_key(ctrl('g'));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Git Diff"));

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git diff"));
}

#[tokio::test]
async fn git_diff_renders_file_drawer_and_highlighted_patch() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old_lib() {}\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn old_main() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new_lib() {}\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn new_main() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let removed_background = renderer.theme().diff_removed_bg;
    let added_background = renderer.theme().diff_added_bg;

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff · Both"), "{viewport}");
    assert!(viewport.contains("lib.rs"), "{viewport}");
    assert!(viewport.contains("main.rs"), "{viewport}");
    assert!(viewport.contains("old_lib") && viewport.contains("new_lib"), "{viewport}");
    assert!(
        viewport.lines().any(|line| line.contains("old_lib") && line.contains("new_lib")),
        "expected wide Git patch to use split layout:\n{viewport}"
    );
    assert!(has_cell(terminal.backend().buffer(), "f", |cell| {
        cell.bg == removed_background && cell.fg != renderer.theme().diff_removed_fg
    }));
    assert!(has_cell(terminal.backend().buffer(), "f", |cell| {
        cell.bg == added_background && cell.fg != renderer.theme().diff_added_fg
    }));

    app.on_key(key(KeyCode::Char('j')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("new_main"));
}

#[tokio::test]
async fn git_diff_cycles_scope_and_stages_selected_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("tracked.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("tracked.rs"), "fn new() {}\n").unwrap();
    std::fs::write(root.join("untracked.rs"), "fn scratch() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(100);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    let status = run_git(root, &["status", "--porcelain"]);
    assert!(status.contains("M  tracked.rs"), "{status}");

    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff · Unstaged"), "{viewport}");
    assert!(viewport.contains("untracked.rs"), "{viewport}");
}

#[tokio::test]
async fn git_diff_stages_selected_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/a.rs"), "fn changed_a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn changed_b() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;

    let status = run_git(root, &["status", "--porcelain"]);
    assert!(status.contains("M  src/a.rs"), "{status}");
    assert!(status.contains("M  src/b.rs"), "{status}");
}

#[tokio::test]
async fn git_diff_reports_non_repository_without_blocking_close() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal_with_width(80);

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[test]
fn required_elicitation_field_must_be_completed() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, mut response_rx) = peer.fake_elicitation(&cx).await;
        let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }))
        .unwrap();

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: schema,
                },
            },
            responder,
        });
        app.on_key(key(KeyCode::Enter));
        assert!(response_rx.try_recv().is_err());
        type_text(&mut app, "Ada");
        app.on_key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({ "name": "Ada" })));
    });
}

#[test]
fn elicitation_request_is_accepted_interactively() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Confirm the action".to_string(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
        assert!(app.has_modal());

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({})));
        assert!(!app.has_modal());
    });
}
