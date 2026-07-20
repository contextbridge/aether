use acp_utils::ElicitationSchema;
use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::notifications::{
    ContextUsage, ContextUsageParams, CreateElicitationRequestParams, ElicitationAction, ElicitationParams,
};
use acp_utils::testing::test_connection;
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
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
use wisp_next::workspace_status::WorkspaceStatus;

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

    let viewport = terminal.backend().buffer();
    assert!(buffer_text(viewport).contains("# Heading"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("Heading"));
    assert!(has_cell(viewport, "H", |cell| cell.fg == heading && cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(viewport, "b", |cell| cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(viewport, "i", |cell| cell.modifier.contains(Modifier::ITALIC)));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let scrollback = buffer_text(terminal.backend().scrollback());
    assert_eq!(scrollback.matches("Heading").count(), 1);
    assert!(!scrollback.contains("**boldword**"));
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

    let scrollback = terminal.backend().scrollback();
    assert!(buffer_text(scrollback).contains("fn highlighted()"));
    assert!(has_cell(scrollback, "f", |cell| cell.bg == code_background));
}

#[test]
fn completed_tool_diff_is_themed_and_inserted_once() {
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

    let scrollback = terminal.backend().scrollback();
    let text = buffer_text(scrollback);
    assert_eq!(text.matches("old_name").count(), 1);
    assert_eq!(text.matches("new_name").count(), 1);
    assert!(has_cell(scrollback, "-", |cell| cell.bg == removed_background));
    assert!(has_cell(scrollback, "+", |cell| cell.bg == added_background));
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

    let text = buffer_text(terminal.backend().scrollback());
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

    let text = buffer_text(terminal.backend().scrollback());
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

    let styled_rows = rows_with_background(terminal.backend().scrollback(), user_background);
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
fn large_markdown_history_is_inserted_in_order_across_chunks() {
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

    let text = buffer_text(terminal.backend().scrollback());
    assert_eq!(text.matches("paragraph-0").count(), 1);
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

fn make_terminal() -> Terminal<TestBackend> {
    make_terminal_with_width(40)
}

fn make_terminal_with_width(width: u16) -> Terminal<TestBackend> {
    Terminal::with_options(TestBackend::new(width, 15), TerminalOptions { viewport: Viewport::Inline(15) }).unwrap()
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
fn user_message_lands_in_scrollback_immediately() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    submit_prompt(&mut app, "hello scrollback");
    sync_terminal(&mut terminal, &mut app).unwrap();

    assert!(buffer_text(terminal.backend().scrollback()).contains("hello scrollback"));
}

#[test]
fn streaming_text_stays_live_until_prompt_done_then_lands_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "hi");

    app.on_acp_event(text_chunk("streamed "));
    app.on_acp_event(text_chunk("answer"));
    sync_terminal(&mut terminal, &mut app).unwrap();

    assert!(buffer_text(terminal.backend().buffer()).contains("streamed answer"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("streamed answer"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal(&mut terminal, &mut app).unwrap();
    sync_terminal(&mut terminal, &mut app).unwrap();

    let scrollback = buffer_text(terminal.backend().scrollback());
    assert_eq!(scrollback.matches("streamed answer").count(), 1);
    assert!(!buffer_text(terminal.backend().buffer()).contains("streamed answer"));
}

#[test]
fn running_tool_holds_later_content_out_of_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run a tool");

    app.on_acp_event(tool_call("tool-1", "Reading main.rs"));
    app.on_acp_event(text_chunk("tool output summary"));
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(terminal.backend().buffer());
    assert!(viewport.contains("Reading main.rs"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("Reading main.rs"));

    app.on_acp_event(tool_completed("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal(&mut terminal, &mut app).unwrap();

    let scrollback = buffer_text(terminal.backend().scrollback());
    assert!(scrollback.contains("Reading main.rs"));
    assert!(scrollback.contains("tool output summary"));
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
fn composer_echo_and_status_line_render_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    type_text(&mut app, "typing");
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(terminal.backend().buffer());
    assert!(viewport.contains("> typing"));
    assert!(viewport.contains("~/code/demo · main"));
    assert!(viewport.contains("aether"));
}

#[test]
fn elicitation_request_is_auto_cancelled() {
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
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert!(response.content.is_none());
    });
}
