use acp_utils::ElicitationSchema;
use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use acp_utils::notifications::{
    AetherCapabilities, AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, ContextUsage,
    ContextUsageParams, CreateElicitationRequestParams, ElicitationAction, ElicitationParams, SessionPreviewResponse,
    SessionPreviewRole, SessionPreviewTurn, WorkspaceEntry, WorkspaceListResponse, WorkspaceMoveResponse,
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::LocalSet;
use wisp_next::app::{App, AppConfig, HistoryItem};
use wisp_next::composer::Composer;
use wisp_next::picker::CommandEntry;
use wisp_next::presentation::TranscriptRenderer;
use wisp_next::render::sync_terminal as sync_terminal_with_renderer;
use wisp_next::screen_router::ScreenEvent;
use wisp_next::screens::git_diff::GitDiffEvent;
use wisp_next::settings::UiSettings;
use wisp_next::theme::Theme;
use wisp_next::{inline_viewport_height, workspace_status::WorkspaceStatus};

fn make_app() -> (App, UnboundedReceiver<PromptCommand>) {
    make_app_in(std::path::PathBuf::from("."))
}

fn make_app_in(working_dir: std::path::PathBuf) -> (App, UnboundedReceiver<PromptCommand>) {
    make_app_with_metadata(working_dir, acp::SessionCapabilities::new(), Vec::new(), Vec::new())
}

fn make_app_with_metadata(
    working_dir: std::path::PathBuf,
    session_capabilities: acp::SessionCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = build_app_with_handle(working_dir, session_capabilities, config_options, auth_methods, prompt_handle);
    (app, command_rx)
}

fn make_failable_app() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
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

fn make_failable_app_with_prompt_search() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
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

fn build_app_with_handle(
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

fn select_option(id: &str, current_value: &str) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        id.to_string(),
        id.to_string(),
        current_value.to_string(),
        vec![acp::SessionConfigSelectOption::new(current_value.to_string(), current_value.to_string())],
    )
}

fn reasoning_option(current: &str, levels: &[&str]) -> acp::SessionConfigOption {
    let options: Vec<acp::SessionConfigSelectOption> =
        levels.iter().map(|&level| acp::SessionConfigSelectOption::new(level.to_string(), level.to_string())).collect();
    acp::SessionConfigOption::select("reasoning_effort", "Reasoning", current.to_string(), options)
}

fn mode_option(current: &str, modes: &[&str]) -> acp::SessionConfigOption {
    let options: Vec<acp::SessionConfigSelectOption> =
        modes.iter().map(|&mode| acp::SessionConfigSelectOption::new(mode.to_string(), mode.to_string())).collect();
    acp::SessionConfigOption::select("mode", "Mode", current.to_string(), options)
        .category(acp::SessionConfigOptionCategory::Mode)
}

#[test]
fn app_exposes_initial_config_option_selections() {
    let options =
        vec![select_option("model", "opus"), select_option("mode", "plan"), select_option("reasoning", "high")];
    let (app, _command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    let selections: Vec<_> = app
        .config_options()
        .iter()
        .map(|option| {
            (
                option.id.0.as_ref(),
                match &option.kind {
                    acp::SessionConfigKind::Select(select) => select.current_value.0.as_ref(),
                    _ => panic!("expected select option"),
                },
            )
        })
        .collect();

    assert_eq!(selections, [("model", "opus"), ("mode", "plan"), ("reasoning", "high")]);
}

#[test]
fn config_option_update_replaces_current_selections() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "opus")],
        Vec::new(),
    );

    app.on_acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
        select_option("mode", "code"),
    ]))));

    assert_eq!(app.config_options().len(), 2);
    let acp::SessionConfigKind::Select(model) = &app.config_options()[0].kind else {
        panic!("expected model select option");
    };
    assert_eq!(model.current_value.0.as_ref(), "sonnet");
}

#[test]
fn auth_methods_update_replaces_current_auth_methods() {
    let initial = vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("initial", "Initial"))];
    let (mut app, _command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), Vec::new(), initial);
    let updated = vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("updated", "Updated"))];

    app.on_acp_event(AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: updated }));

    assert_eq!(app.auth_methods().len(), 1);
    assert_eq!(app.auth_methods()[0].id().0.as_ref(), "updated");
}

#[test]
fn session_capability_metadata_enables_only_advertised_features() {
    let session_capabilities = acp::SessionCapabilities::new()
        .meta(Some(AetherCapabilities { prompt_search: true, session_preview: false, workspace_move: true }.to_meta()));
    let (app, _command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new());

    assert!(app.supports_prompt_search());
    assert!(!app.supports_session_preview());
    assert!(app.supports_workspace_move());
}

#[test]
fn composer_soft_wraps_and_tracks_cursor() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefgh");
    composer.move_left();
    composer.move_left();

    let layout = composer.layout(6, &Theme::default());

    assert_eq!(layout.lines.len(), 4);
    assert_eq!(layout.cursor.x, 4);
    assert_eq!(layout.cursor.y, 2);
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
            builtin: false,
        },
        CommandEntry {
            name: "status".to_string(),
            description: "Show status".to_string(),
            has_input: false,
            hint: None,
            builtin: false,
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
    let (text, pending) = composer.take_submission();
    assert_eq!(text, "first");
    assert!(pending.is_empty());
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
    let mut terminal = make_terminal_with_width(160);
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
    let mut terminal = make_terminal_with_width(160);
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
fn settings_deserializes_status_line() {
    let settings: UiSettings = serde_json::from_str(
        r#"{"contentPadding":4,"theme":{"file":"nord.tmTheme","future":true},"statusLine":{"left":["cwd"],"right":["agent"]}}"#,
    )
    .unwrap();

    assert_eq!(settings.content_padding, Some(4));
    assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
    assert!(settings.status_line.is_some());
    let sl = settings.status_line.unwrap();
    assert_eq!(sl.left, Some(vec![wisp_next::settings::StatusLineSegmentConfig::Cwd { max_width: None }]));
    assert_eq!(sl.right, Some(vec![wisp_next::settings::StatusLineSegmentConfig::Agent]));
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

fn make_terminal_tall() -> Terminal<TestBackend> {
    make_terminal_with_dimensions(80, 30)
}

fn make_terminal_with_dimensions(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::with_options(
        TestBackend::new(width, height),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(height)) },
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

fn tool_call_with_raw(id: &str, title: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title).raw_input(raw_input)))
}

fn tool_call_update_with_raw(id: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().raw_input(raw_input),
    )))
}

#[test]
fn streamed_raw_input_fragments_accumulate_and_show_after_completion() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));
    app.on_acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("first ".to_string())));
    app.on_acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("second".to_string())));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("first second"), "streamed fragments must appear: {viewport}");
}

fn tool_call_update_with_display_value(id: &str, display_value: &str) -> AcpEvent {
    let mut meta = serde_json::Map::new();
    meta.insert("display_value".to_string(), serde_json::Value::String(display_value.to_string()));
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new()).meta(meta),
    ))
}

fn tool_completed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn tool_failed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Failed),
    )))
}

#[test]
fn running_tool_hides_raw_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Read file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden while running: {viewport}");
}

#[test]
fn completed_tool_shows_raw_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/src/main.rs"), "raw args must be visible after completion: {viewport}");
}

#[test]
fn display_value_overrides_raw_arguments_in_rendered_output() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_call_update_with_display_value("tool-1", "42 lines read"));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("42 lines read"), "display_value must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden when display_value is set: {viewport}");
}

#[test]
fn error_cause_is_visible_in_rendered_output() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Failing tool"));
    app.on_acp_event(tool_failed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("failed"), "error cause must be visible: {viewport}");
    assert!(!viewport.contains("(failed)"), "error cause must NOT have parentheses: {viewport}");
}

#[test]
fn truncation_adds_visible_ellipsis_for_long_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let long = "x".repeat(300);
    app.on_acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains('…'), "truncated args must show ellipsis: {viewport}");
}

#[test]
fn truncation_keeps_short_arguments_unchanged() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let short = "hello world";
    app.on_acp_event(tool_call_with_raw("tool-1", "Short arg", serde_json::Value::String(short.to_string())));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains(short), "short args must appear in full: {viewport}");
    assert!(!viewport.contains('…'), "short args must NOT have ellipsis: {viewport}");
}

#[test]
fn truncation_is_unicode_safe_no_split_characters() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let prefix = "a".repeat(195);
    let unicode_arg = format!("{prefix}こんにちは世界");
    app.on_acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(unicode_arg)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains('…'), "truncated Unicode args must show ellipsis: {viewport}");
    assert!(viewport.contains('ん'), "char-based truncation must preserve leading multi-byte chars: {viewport}");
    assert!(!viewport.contains('ち'), "truncation must stop at char boundary (199 chars): {viewport}");
}

#[test]
fn char_based_truncation_preserves_multi_byte_under_200_chars() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    // 100 ASCII chars + 50 × '界' (3 bytes each) = 150 chars, but 100 + 150 = 250 bytes
    // This is under 200 chars so it must NOT be truncated even though it exceeds 200 bytes.
    let half = "a".repeat(100);
    let unicode_half = "界".repeat(50);
    let arg = format!("{half}{unicode_half}");
    assert!(arg.len() > 200, "arg must exceed 200 bytes for this test");
    assert!(arg.chars().count() < 200, "arg must be under 200 chars (with leading space)");
    app.on_acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(arg)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains('…'), "under 200 chars must not be truncated: {viewport}");
    assert!(viewport.contains('界'), "multi-byte chars must appear in full: {viewport}");
}

#[test]
fn truncation_boundary_exactly_200_bytes_no_ellipsis() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let exactly_199 = "x".repeat(199);
    app.on_acp_event(tool_call_with_raw("tool-1", "199-char arg", serde_json::Value::String(exactly_199)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains('…'), "199-char args must not be truncated (fit in 200 with space): {viewport}");
}

#[test]
fn truncation_preserved_in_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_dimensions(250, 15);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "run tool");
    let long = "x".repeat(300);
    app.on_acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    app.on_acp_event(tool_completed_status("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(conversation.contains('…'), "truncation must survive drain to scrollback: {conversation}");
}

#[test]
fn tool_arguments_preserved_in_scrollback_exactly_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let raw = serde_json::json!({"path": "/src/main.rs"});
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_completed_status("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    let occurrences = conversation.matches("/src/main.rs").count();
    assert_eq!(occurrences, 1, "tool args must appear exactly once in conversation: {conversation}");
    assert!(conversation.contains("Read file"), "title must appear: {conversation}");
}

#[test]
fn diff_not_rendered_while_tool_is_running() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new().content(vec![acp::ToolCallContent::Diff(diff)]),
    );
    app.on_acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Edit file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("old content"), "diff must NOT render while running: {viewport}");
    assert!(!viewport.contains("new content"), "diff must NOT render while running: {viewport}");
}

#[test]
fn diff_not_rendered_after_failed_status() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Failed),
    );
    app.on_acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(!conversation.contains("old content"), "diff must NOT render after failure: {conversation}");
    assert!(!conversation.contains("new content"), "diff must NOT render after failure: {conversation}");
    assert!(conversation.contains("failed"), "error cause must be visible: {conversation}");
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
    assert_eq!(message_row + 2, composer_row);
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
    let mut terminal = make_terminal_with_width(160);
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

#[tokio::test]
async fn git_diff_commit_disabled_with_nothing_staged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "content\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Nothing staged to commit"), "{viewport}");

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_success() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "my commit message");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    let log = run_git(root, &["log", "--oneline", "-1"]);
    assert!(log.contains("my commit message"), "log was: {log}");
}

#[tokio::test]
async fn git_diff_commit_empty_message_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Commit message cannot be empty"), "{viewport}");
}

#[tokio::test]
async fn git_diff_commit_esc_cancels() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "should not commit");
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff"), "should still be on diff screen:\n{viewport}");
}

#[tokio::test]
async fn git_diff_discard_confirmation_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Discard changes to"), "{viewport}");
    assert!(viewport.contains("file.txt"), "{viewport}");

    app.on_key(key(KeyCode::Char('n')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Discard"), "discard prompt should be gone:\n{viewport}");

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "changed\n");
}

#[tokio::test]
async fn git_diff_discard_reverts_modified_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "original\n");
}

#[tokio::test]
async fn git_diff_discard_removes_untracked_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("tracked.txt"), "v1\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("untracked.txt"), "scratch\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    assert!(!root.join("untracked.txt").exists());
}

#[tokio::test]
async fn git_diff_discard_restores_deleted_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::remove_file(root.join("file.txt")).unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "original\n");
}

#[tokio::test]
async fn git_diff_full_file_toggle_shows_content() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/main.rs"), "fn new() {}\nfn extra() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("[full file]"), "{viewport}");
    assert!(viewport.contains("fn new()"), "{viewport}");
    assert!(viewport.contains("fn extra()"), "{viewport}");

    app.on_key(key(KeyCode::Char('o')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("[full file]"), "{viewport}");
    assert!(viewport.contains("fn old()"), "{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_shows_deleted_message() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "content\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::remove_file(root.join("file.txt")).unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("deleted"), "{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_toggle_at_narrow_width() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.rs"), "fn new() {}\nfn two() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(50);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("fn new()"), "{viewport}");
    assert!(viewport.contains("fn two()"), "{viewport}");
}

#[tokio::test]
async fn git_diff_stale_event_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.rs"), "fn one() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.rs"), "fn changed() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    let stale_event = GitDiffEvent::ActionFinished { request_id: 0, result: Ok(()) };
    app.on_screen_event(ScreenEvent::GitDiff(stale_event));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_screen_closable_on_error() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal_with_width(80);

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_failure_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join(".git/hooks")).ok();
    std::fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\necho nope >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join(".git/hooks/pre-commit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init", "--no-verify"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    type_text(&mut app, "should fail");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("nope") || viewport.contains("CommandFailed") || viewport.contains("failed"),
        "expected commit error in viewport:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_binary_file_shows_label() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("image.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("image.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Binary file"), "expected binary file label in:\n{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_binary_shows_message() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("data.bin"), b"\x00\x01\x02\x03").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("data.bin"), b"\x00\x01\x02\x03\x04").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("Binary file") || viewport.contains("binary"),
        "expected binary message in full-file mode:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_full_file_load_error_exits_full_file_mode() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("source.rs"), "fn answer() -> u32 { 42 }\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("source.rs"), "fn answer() -> u32 { 43 }\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("[full file]"), "{viewport}");

    std::fs::remove_file(root.join("source.rs")).unwrap();
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("[full file]"), "should exit full-file mode on load error:\n{viewport}");
    assert!(
        viewport.contains("Cannot read") || viewport.contains("source.rs"),
        "should show error in footer:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_editor_unicode_cursor_and_render() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "héllo wörld — café");
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo wörld — café"), "expected unicode commit message:\n{viewport}");

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_comment_draft_submit_cancel_undo() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new() {}\nfn another() {}\nfn third() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Navigate into patch pane: move to file, then enter
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Press 'c' to start draft on first line
    app.on_key(key(KeyCode::Char('c')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Draft"), "draft should appear:\n{viewport}");

    // Type a comment
    type_text(&mut app, "this looks wrong");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("this looks wrong"), "typed text should appear in draft:\n{viewport}");

    // Submit the comment
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Comment"), "submitted comment should appear:\n{viewport}");
    assert!(viewport.contains("this looks wrong"), "submitted text should be visible:\n{viewport}");

    // Undo the comment
    app.on_key(key(KeyCode::Char('u')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("this looks wrong"), "comment should be removed after undo:\n{viewport}");

    // Esc cancels draft
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "will cancel");
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("will cancel"), "cancelled draft should not appear:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_counts_in_footer() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\nfn two() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\nfn two() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("1 comment")), "file header should show comment count:\n{viewport}");

    // Footer should show total count - check the raw footer area
    assert!(viewport.contains("(1 comment)"), "footer should show (1 comment):\n{viewport}");
}

#[tokio::test]
async fn git_diff_comments_survive_file_switches() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("a.rs"), "fn a_old() {}\n").unwrap();
    std::fs::write(root.join("b.rs"), "fn b_old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("a.rs"), "fn a_new() {}\n").unwrap();
    std::fs::write(root.join("b.rs"), "fn b_new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add comment on first file (a.rs)
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "comment on A");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("comment on A"), "comment on A should be visible:\n{viewport}");

    // Switch to drawer and select second file
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("comment on A"), "comment on A should not appear on B:\n{viewport}");

    // Switch back to first file
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('k')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("comment on A"), "comment on A should persist after switching back:\n{viewport}");
}

#[tokio::test]
async fn git_diff_submit_review_emits_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, mut command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback text");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Submit
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Verify the prompt was sent
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert!(text.contains("I'm reviewing the working tree diff"), "text should contain review prefix:\n{text}");
            assert!(text.contains("## `lib.rs`"), "text should contain file header:\n{text}");
            assert!(text.contains("feedback text"), "text should contain comment body:\n{text}");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }

    // Screen should be closed after successful submit
    assert!(!app.full_screen_active(), "screen should close after submit");
}

#[tokio::test]
async fn git_diff_submit_no_comments_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Press 's' with no comments
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("No comments to submit"), "should show error for no comments:\n{viewport}");
}

#[tokio::test]
async fn git_diff_submit_send_failure_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, fail_signal, _command_rx) = make_failable_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback text");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Enable send failure
    fail_signal.store(true, Ordering::SeqCst);

    // Try to submit
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;

    // The screen should still be active (failure retains state)
    assert!(app.full_screen_active(), "screen should remain open after send failure");

    // Comments should still be visible
    app.on_key(key(KeyCode::Esc)); // close screen to check transcript
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Failed to send review"), "should show send failure message in transcript:\n{viewport}");
}

fn make_failable_app_in(working_dir: std::path::PathBuf) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app =
        build_app_with_handle(working_dir, acp::SessionCapabilities::new(), Vec::new(), Vec::new(), prompt_handle);
    (app, fail_signal, command_rx)
}

#[tokio::test]
async fn git_diff_comment_refresh_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "keep me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("keep me"), "comment should appear before refresh:\n{viewport}");

    // First 'r' — show confirmation
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation message should appear:\n{viewport}");
    assert!(viewport.contains("keep me"), "comment should still appear during confirmation:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("keep me"), "comment should survive cancel:\n{viewport}");
    assert!(!viewport.contains("will clear"), "confirmation message should be gone:\n{viewport}");

    // Confirm path: press r twice
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("keep me"), "comment should be cleared after confirmed refresh:\n{viewport}");
    assert!(viewport.contains("Git Diff"), "screen should still show diff:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_scope_switch_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "scope comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 't' — show confirmation
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("scope comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("scope comment"), "comment should be cleared after scope switch:\n{viewport}");
    assert!(viewport.contains("Git Diff"), "screen should still show diff:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_stage_all_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "stage me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'a' — show confirmation
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("stage me"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("stage me"), "comment should be cleared after stage-all:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_toggle_stage_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "space comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First space — show confirmation
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("space comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("space comment"), "comment should be cleared after stage:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_commit_cancel_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "commit comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'C' — show confirmation (even though nothing staged, confirm appears first)
    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("commit comment"), "comment should survive cancel:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_discard_cancel_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "discard comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'd' — show confirmation
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("discard comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path — should enter discard confirmation
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Discard changes"), "discard confirmation should appear:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_unstage_all_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    // Switch to staged scope to see the staged file
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "unstage me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'A' — show confirmation
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("unstage me"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("unstage me"), "comment should be cleared after unstage-all:\n{viewport}");
}

#[tokio::test]
async fn git_diff_draft_cursor_with_unicode() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Start draft and type unicode text
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "héllo wörld");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo wörld"), "unicode text should appear in draft:\n{viewport}");

    // Move cursor left and type more
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    type_text(&mut app, "★");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo★ wörld"), "unicode insertion should work:\n{viewport}");
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

#[test]
fn tab_cycles_reasoning_effort_through_advertised_levels() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, mut command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "medium")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "high")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "none")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "low")
    );
}

#[test]
fn backtab_cycles_mode_option_and_wraps() {
    let options = vec![mode_option("code", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "plan")
    );

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "ask")
    );

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "code")
    );
}

#[test]
fn tab_and_backtab_noop_without_cycleable_options() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));

    assert!(command_rx.try_recv().is_err());
}

#[test]
fn command_picker_consumes_tab_without_changing_config() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, _command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert!(!app.composer().has_overlay());
}

#[test]
fn failed_config_update_shows_error_and_does_not_corrupt_state() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, mut command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "server error".to_string() });

    let items = app.drain_finalized();
    let has_error = items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("Failed to update")));
    assert!(has_error, "expected user-visible error, got {items:?}");

    let reasoning = app.config_options().iter().find(|o| o.id.0.as_ref() == "reasoning_effort");
    let acp::SessionConfigKind::Select(select) = &reasoning.unwrap().kind else {
        panic!("expected select");
    };
    assert_eq!(select.current_value.0.as_ref(), "medium");
}

fn session_info(id: &str, cwd: &str, title: &str, updated_at: &str) -> acp::SessionInfo {
    acp::SessionInfo::new(SessionId::new(id), cwd)
        .title(Some(title.to_string()))
        .updated_at(Some(updated_at.to_string()))
}

fn sessions_listed(sessions: Vec<acp::SessionInfo>) -> AcpEvent {
    AcpEvent::SessionsListed { sessions }
}

fn session_loaded(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    AcpEvent::SessionLoaded { session_id: SessionId::new(session_id), config_options }
}

fn new_session_created(session_id: &str, config_options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    AcpEvent::NewSessionCreated { session_id: SessionId::new(session_id), config_options }
}

fn session_preview_response(session_id: &str) -> SessionPreviewResponse {
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

fn session_update_for(session_id: &str, update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new(session_id), update: Box::new(update) }
}

fn user_message_chunk(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))))
}

fn make_app_with_session_preview() -> (App, UnboundedReceiver<PromptCommand>) {
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: false, session_preview: true, workspace_move: false }.to_meta(),
    ));
    make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new())
}

fn make_app_with_workspace_move() -> (App, UnboundedReceiver<PromptCommand>) {
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: false, session_preview: false, workspace_move: true }.to_meta(),
    ));
    make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new())
}

fn workspace_entry(path: &str, is_current: bool) -> WorkspaceEntry {
    WorkspaceEntry { path: std::path::PathBuf::from(path), is_current }
}

fn workspaces_listed(workspaces: Vec<WorkspaceEntry>) -> AcpEvent {
    AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces })
}

fn workspace_list_failed(error: &str) -> AcpEvent {
    AcpEvent::WorkspaceListFailed { error: error.to_string() }
}

fn workspace_moved(new_cwd: &str) -> AcpEvent {
    AcpEvent::WorkspaceMoved(WorkspaceMoveResponse { new_cwd: std::path::PathBuf::from(new_cwd) })
}

fn workspace_move_failed(error: &str) -> AcpEvent {
    AcpEvent::WorkspaceMoveFailed { error: error.to_string() }
}

#[test]
fn clear_is_builtin_and_issues_new_session_command() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));

    let cmd = command_rx.try_recv().ok();
    assert!(
        matches!(cmd, Some(PromptCommand::NewSession { .. })),
        "expected NewSession command after /clear, got {cmd:?}"
    );
}

#[test]
fn clear_creates_new_session_and_resets_state() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    submit_prompt(&mut app, "old message");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("old message"));

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    let old_generation = app.transcript_generation();
    app.on_acp_event(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_eq!(app.transcript_generation(), old_generation.wrapping_add(1));
    assert_eq!(app.pending_items().len(), 1);
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("old message"), "old message should be gone after clear:\n{viewport}");
}

#[test]
fn clear_restores_compatible_config_selections() {
    let options = vec![select_option("model", "opus"), mode_option("code", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(new_session_created(
        "new-session",
        vec![select_option("model", "haiku"), mode_option("ask", &["code", "plan", "ask"])],
    ));

    let restore_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&restore_cmd, PromptCommand::SetConfigOption { config_id, value, .. } if config_id == "model" && value == "opus"),
        "expected model restored to opus, got {restore_cmd:?}"
    );
}

#[test]
fn resume_is_builtin_and_lists_sessions() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));

    let cmd = command_rx.try_recv().ok();
    assert!(
        matches!(cmd, Some(PromptCommand::ListSessions)),
        "expected ListSessions command after /resume, got {cmd:?}"
    );
}

#[test]
fn session_list_excludes_active_session() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("test-session", "/tmp/current", "Current", "2025-01-01T00:00:00Z"),
        session_info("other-session", "/tmp/other", "Other", "2025-01-02T00:00:00Z"),
    ]));

    assert!(app.has_session_picker());
}

#[test]
fn resume_loads_selected_session() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));

    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::LoadSession { session_id, cwd } if session_id.0.as_ref() == "old" && cwd == &std::path::PathBuf::from("/tmp/old")),
        "expected LoadSession for old session, got {cmd:?}"
    );
}

#[test]
fn empty_session_list_shows_no_sessions() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![]));

    assert!(app.has_session_picker());
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("No previous sessions"), "expected empty state:\n{viewport}");
}

#[test]
fn esc_closes_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    app.on_key(key(KeyCode::Esc));
    assert!(!app.has_session_picker());
}

// ── Sub-agent integration tests ──

use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest, SubAgentToolResult};

fn sub_agent_progress(parent_tool_id: &str, task_id: &str, agent_name: &str, event: SubAgentEvent) -> AcpEvent {
    AcpEvent::SubAgentProgress(SubAgentProgressParams {
        parent_tool_id: parent_tool_id.to_string(),
        task_id: task_id.to_string(),
        agent_name: agent_name.to_string(),
        event,
    })
}

fn sub_agent_tool_call(parent_id: &str, task_id: &str, agent: &str, tool_id: &str, name: &str, args: &str) -> AcpEvent {
    sub_agent_progress(
        parent_id,
        task_id,
        agent,
        SubAgentEvent::ToolCall {
            request: SubAgentToolRequest {
                id: tool_id.to_string(),
                name: name.to_string(),
                arguments: args.to_string(),
            },
        },
    )
}

fn sub_agent_done(parent_id: &str, task_id: &str, agent: &str) -> AcpEvent {
    sub_agent_progress(parent_id, task_id, agent, SubAgentEvent::Done)
}

#[test]
fn sub_agent_progress_event_is_handled() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));

    // Send a sub-agent ToolCall event - should not crash
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));

    // Parent with running sub-agent is still running
    assert!(app.is_agent_busy());
}

#[test]
fn sub_agent_parent_stays_live_while_child_running() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // parent completed, no sub-agents → should drain
    app.on_acp_event(text_chunk("Done"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    let items = app.drain_finalized();
    // parent should be in the drained items
    assert!(items.iter().any(|item| matches!(item, HistoryItem::Tool { title, .. } if title == "spawn_subagent")));
}

#[test]
fn sub_agent_keeps_agent_busy() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // still busy because prompt_in_flight was never set in this test
    // but the sub-agent running check uses any_running which includes sub-agents
}

#[test]
fn sub_agent_context_cleared_removes_state() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.is_agent_busy());

    app.on_acp_event(AcpEvent::ContextCleared(acp_utils::notifications::ContextClearedParams {}));

    assert!(!app.is_agent_busy());
}

#[test]
fn sub_agent_wants_tick_while_running() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.wants_tick());

    app.on_acp_event(sub_agent_done("parent-1", "task-a", "explorer"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    // after finalization, sub-agent tools are finalized
    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_renders_tree_guides_in_viewport() {
    let (mut app, _command_rx) = make_app();

    // Start a parent tool call and enter prompt mode
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // Add sub-agents with child tools
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c2", "read", r#"{"path":"src/main.rs"}"#));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    // Tree guides should be visible for the sub-agent
    assert!(viewport.contains("explorer"), "viewport should show agent name:\n{viewport}");
}

#[test]
fn sub_agent_drain_includes_sub_agents_in_history_items() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // Add, complete, and mark done a sub-agent
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.on_acp_event(sub_agent_progress(
        "parent-1",
        "task-a",
        "explorer",
        SubAgentEvent::ToolResult {
            result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
        },
    ));
    app.on_acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    // End prompt to finalize everything
    app.on_acp_event(text_chunk("Done"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let items = app.drain_finalized();
    let tool_item = items.iter().find_map(|item| match item {
        HistoryItem::Tool { title, sub_agents, .. } if title == "spawn_subagent" => Some(sub_agents),
        _ => None,
    });

    assert!(tool_item.is_some(), "parent tool should be in drained items");
    let sub_agents = tool_item.unwrap();
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0].agent_name, "explorer");
    assert!(sub_agents[0].done);
    assert_eq!(sub_agents[0].tools.len(), 1);
    assert_eq!(sub_agents[0].tools[0].name, "grep");
}

#[test]
fn sub_agent_prompt_error_finalizes_sub_agents() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    // PromptError should finalize sub-agents
    app.on_acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));

    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_prompt_cancelled_finalizes_sub_agents() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_multiple_sub_agents_per_parent() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-b", "builder", "c2", "write", "{}"));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    // Both agent names should be visible
    assert!(viewport.contains("explorer"), "viewport should show explorer:\n{viewport}");
    assert!(viewport.contains("builder"), "viewport should show builder:\n{viewport}");
}

mod progress_indicator_tests {
    use super::*;
    use wisp_next::progress_indicator::SPINNER_FRAMES;

    #[test]
    fn idle_renders_no_progress_indicator() {
        let (mut app, _command_rx) = make_app();
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(!has_spinner, "buffer should not contain spinner when idle:\n{full}");
        assert!(!full.contains("Working..."), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn prompt_shows_progress_with_esc_hint() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        assert!(
            app.progress_indicator().is_active(),
            "progress indicator not active. prompt_in_flight={}, is_agent_busy={}",
            app.busy(),
            app.is_agent_busy()
        );
        // Use a 120-char terminal to fit the full tip + esc hint on one line
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(has_spinner, "full buffer should contain spinner during prompt:\n{full}");
        assert!(full.contains("esc to interrupt"), "full buffer should show esc hint:\n{full}");
    }

    #[test]
    fn prompt_done_hides_progress() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(text_chunk("response"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(!full.contains("esc to interrupt"), "{full}");
        assert!(!full.contains("Working..."), "{full}");
    }

    #[test]
    fn compaction_active_shows_compacting_message() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_inactive_hides_indicator() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: false }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_during_prompt_shows_esc_hint() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(full.contains("Compacting context"), "{full}");
        assert!(full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_moving_shows_progress() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.on_key(key(KeyCode::Enter));
        let _ = command_rx.try_recv().unwrap();
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(full.contains("Moving workspace"), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_loading_session_shows_progress() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.on_key(key(KeyCode::Enter));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspace_moved("/home/user/code/other"));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Loading session in new workspace"), "{viewport}");
    }

    #[test]
    fn workspace_move_failure_clears_indicator() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.on_key(key(KeyCode::Enter));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspace_move_failed("permission denied"));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Loading session"), "{viewport}");
    }

    #[test]
    fn workspace_move_precedence_over_compaction() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.on_key(key(KeyCode::Enter));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn compaction_precedence_over_agent_work() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn wants_tick_true_immediately_after_prompt_submit() {
        let (mut app, _command_rx) = make_app();
        assert!(!app.wants_tick(), "wants_tick should be false when idle");
        submit_prompt(&mut app, "hello");
        assert!(app.wants_tick(), "wants_tick should be true immediately after prompt submit (raw state)");
    }

    #[test]
    fn wants_tick_true_during_compaction() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        assert!(app.wants_tick(), "wants_tick should be true during compaction");
    }

    #[test]
    fn wants_tick_true_during_workspace_move() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.on_key(key(KeyCode::Enter));
        let _ = command_rx.try_recv().unwrap();
        assert!(app.wants_tick(), "wants_tick should be true during workspace move");
    }

    #[test]
    fn wants_tick_false_after_idle() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(text_chunk("done"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        assert!(!app.wants_tick(), "wants_tick should be false after prompt completes and no other activity");
    }

    #[test]
    fn tick_animates_spinner_deterministically() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");

        let now = Instant::now();

        // Capture rendering at tick 0
        let mut terminal_a = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal_a, &mut app).unwrap();
        let full_a = buffer_text(terminal_a.backend().buffer());

        // Advance tick once
        app.on_tick(now);
        let mut terminal_b = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal_b, &mut app).unwrap();
        let full_b = buffer_text(terminal_b.backend().buffer());

        // Different frames should produce different braille characters
        assert_ne!(full_a, full_b, "spinner should animate with each tick");
    }

    #[test]
    fn tick_stops_when_idle() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");

        let now = Instant::now();
        app.on_tick(now);

        // Complete the prompt
        app.on_acp_event(text_chunk("response"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        // Tick while idle — should not change
        app.on_tick(now);
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn progress_is_not_inserted_into_scrollback() {
        let (mut app, _command_rx) = make_app();
        // Fill enough content to trigger scrollback
        submit_prompt(&mut app, "hello");
        let mut response = String::new();
        for i in 0..20 {
            writeln!(response, "line-{i}").unwrap();
        }
        app.on_acp_event(text_chunk(&response));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        // Submit another prompt so we can see progress indicator
        submit_prompt(&mut app, "another");
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();

        let scrollback = buffer_text(&history_buffer(&mut terminal));
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| scrollback.contains(frame));
        assert!(!has_spinner, "scrollback should not contain progress spinner:\n{scrollback}");
        assert!(!scrollback.contains("esc to interrupt"), "scrollback should not contain esc hint:\n{scrollback}");
        assert!(!scrollback.contains("Working..."), "scrollback should not contain Working:\n{scrollback}");
    }

    #[test]
    fn context_cleared_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams {}));

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!viewport.contains("esc to interrupt"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after context cleared");
    }

    #[test]
    fn new_session_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::NewSessionCreated {
            session_id: SessionId::new("new-session"),
            config_options: Vec::new(),
        });

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after new session");
    }

    #[test]
    fn session_loaded_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::SessionLoaded {
            session_id: SessionId::new("loaded-session"),
            config_options: Vec::new(),
        });

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn progress_lines_include_padding() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();

        let full = buffer_text(terminal.backend().buffer());
        let spinner_lines: Vec<_> =
            full.lines().filter(|line| SPINNER_FRAMES.iter().any(|frame| line.contains(frame))).collect();
        assert!(!spinner_lines.is_empty(), "should have spinner lines:\n{full}");
        for line in spinner_lines {
            assert!(line.starts_with("  "), "spinner line should start with padding spaces, got: '{line}'");
        }
    }

    #[test]
    fn workspace_list_failed_clears_indicator() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspace_list_failed("network error"));

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after list failure");
    }
}

// --- Plan ACP-event integration tests ---

fn plan_entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content.to_string(), acp::PlanEntryPriority::Medium, status)
}

fn plan_update(entries: Vec<acp::PlanEntry>) -> AcpEvent {
    session_update(acp::SessionUpdate::Plan(acp::Plan::new(entries)))
}

#[test]
fn plan_renders_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_tall();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Research", acp::PlanEntryStatus::InProgress),
        plan_entry("Implement", acp::PlanEntryStatus::Pending),
    ]));

    assert!(app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "viewport should show Plan header:\n{viewport}");
    assert!(viewport.contains("Research"), "viewport should show Research:\n{viewport}");
    assert!(viewport.contains("Implement"), "viewport should show Implement:\n{viewport}");
}

#[test]
fn plan_ordering_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_tall();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Completed task", acp::PlanEntryStatus::Completed),
        plan_entry("Pending task", acp::PlanEntryStatus::Pending),
        plan_entry("InProgress task", acp::PlanEntryStatus::InProgress),
    ]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    let in_progress_pos = viewport.find("InProgress task").unwrap();
    let pending_pos = viewport.find("Pending task").unwrap();
    let completed_pos = viewport.find("Completed task").unwrap();

    assert!(in_progress_pos < pending_pos, "InProgress should render before Pending:\n{viewport}");
    assert!(pending_pos < completed_pos, "Pending should render before Completed:\n{viewport}");
}

#[test]
fn plan_grace_period_hides_completed() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let now = Instant::now();

    app.plan_tracker_mut().replace(vec![plan_entry("Done", acp::PlanEntryStatus::Completed)], now);
    app.plan_tracker_mut().on_tick(now);

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Done"), "completed entry visible at t=0:\n{viewport}");

    app.plan_tracker_mut().on_tick(now + Duration::from_secs(5));
    app.on_key(key(KeyCode::Enter));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport_after = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport_after.contains("Done"), "completed entry hidden after 5s grace:\n{viewport_after}");
}

#[test]
fn plan_grace_period_timestamp_preserved_across_repeated_updates() {
    let (mut app, _command_rx) = make_app();

    let now = Instant::now();
    app.plan_tracker_mut().replace(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)], now);

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)]));

    app.plan_tracker_mut().on_tick(now);
    assert!(app.has_plan(), "still visible at original completion time");

    app.plan_tracker_mut().on_tick(now + Duration::from_secs(10));
    let entries = app.plan_entries().to_vec();
    assert!(entries.is_empty(), "hidden when original timestamp exceeds grace");
}

#[test]
fn plan_coexists_with_streaming_transcript() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Research", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "explain");
    app.on_acp_event(text_chunk("Here is the explanation."));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "plan header visible:\n{viewport}");
    assert!(viewport.contains("Research"), "plan entry visible:\n{viewport}");
    assert!(viewport.contains("Here is the explanation."), "transcript visible:\n{viewport}");
}

#[test]
fn plan_coexists_with_tool_calls() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Edit files", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "fix it");
    app.on_acp_event(tool_call("tool-1", "Editing main.rs"));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Edit files"), "plan entry visible:\n{viewport}");
    assert!(viewport.contains("Editing main.rs"), "tool call visible:\n{viewport}");
}

#[test]
fn plan_short_terminal_clips_plan() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(40);
    terminal.backend_mut().resize(40, 8);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Plan item one", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item two", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item three", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item four", acp::PlanEntryStatus::Pending),
    ]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "plan header visible on short terminal:\n{viewport}");
    let visible_entries = vec!["Plan item one", "Plan item two", "Plan item three", "Plan item four"]
        .into_iter()
        .filter(|e| viewport.contains(e))
        .count();
    assert!(visible_entries < 4, "short terminal should clip plan, but all 4 entries visible:\n{viewport}");
}

#[test]
fn plan_not_in_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("A plan task", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "hello");
    app.on_acp_event(text_chunk("response text"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let scrollback = buffer_text(&history_buffer(&mut terminal));

    assert!(!scrollback.contains("A plan task"), "plan should not be in scrollback:\n{scrollback}");
}

#[test]
fn plan_cleared_on_context_cleared() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after context clear:\n{viewport}");
}

#[test]
fn plan_cleared_on_new_session() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::NewSessionCreated { session_id: SessionId::new("new-id"), config_options: Vec::new() });
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after new session:\n{viewport}");
}

#[test]
fn plan_cleared_on_session_loaded() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::SessionLoaded {
        session_id: SessionId::new("other-session"),
        config_options: Vec::new(),
    });
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after session load:\n{viewport}");
}

// --- MCP Server Status & Provider Login Tests ---

use acp_utils::notifications::{McpNotification, McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

fn server_status_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status)
}

fn oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_auth_capability(McpServerAuthCapability::OAuth)
}

fn mcp_notification(servers: Vec<McpServerStatusEntry>) -> AcpEvent {
    AcpEvent::McpNotification(McpNotification::ServerStatus { servers })
}

fn auth_complete(method_id: &str) -> AcpEvent {
    AcpEvent::AuthenticateComplete { method_id: method_id.to_string() }
}

fn auth_failed(method_id: &str) -> AcpEvent {
    AcpEvent::AuthenticateFailed { method_id: method_id.to_string(), error: "simulated failure".to_string() }
}

fn auth_method(id: &str, name: &str, description: Option<&str>) -> acp::AuthMethod {
    let mut agent = acp::AuthMethodAgent::new(id.to_string(), name);
    if let Some(desc) = description {
        agent = agent.description(desc);
    }
    acp::AuthMethod::Agent(agent)
}

fn auth_methods_updated(methods: Vec<acp::AuthMethod>) -> AcpEvent {
    AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: methods })
}

#[test]
fn server_status_unhealthy_count_updates_status_line() {
    let (mut app, _command_rx) = make_app();
    assert_eq!(app.unhealthy_server_count(), 0);

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    assert_eq!(app.unhealthy_server_count(), 2);
}

#[test]
fn server_status_all_connected_gives_zero_unhealthy() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Connected { tool_count: 2 }),
    ]));

    assert_eq!(app.unhealthy_server_count(), 0);
}

#[test]
fn server_status_empty_clears_count() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "x",
        McpServerStatus::Failed { error: "e".to_string() },
    )]));
    assert_eq!(app.unhealthy_server_count(), 1);

    app.on_acp_event(mcp_notification(vec![]));
    assert_eq!(app.unhealthy_server_count(), 0);
}

#[test]
fn settings_overlay_shows_mcp_servers_entry() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "github",
        McpServerStatus::Connected { tool_count: 3 },
    )]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("MCP Servers"), "settings should show MCP Servers entry:\n{viewport}");
}

#[test]
fn settings_overlay_shows_provider_logins_when_auth_methods_present() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("codex", "Codex", None)],
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Provider Logins"), "settings should show Provider Logins:\n{viewport}");
}

#[test]
fn settings_overlay_no_provider_logins_when_auth_methods_empty() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Provider Logins"), "should not show Provider Logins when empty:\n{viewport}");
}

#[test]
fn mcp_server_status_pane_renders_entries() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        oauth_server("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    // Navigate to MCP Servers entry (after Theme entry)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("5 tools"), "should show tool count:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
    assert!(viewport.contains("needs authentication"), "should show auth needed:\n{viewport}");
    assert!(viewport.contains("slack"), "should show slack:\n{viewport}");
    assert!(viewport.contains("timeout"), "should show error:\n{viewport}");
}

#[test]
fn mcp_server_status_empty_shows_placeholder() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("no MCP servers configured"), "should show placeholder:\n{viewport}");
}

#[test]
fn selecting_oauth_server_emits_authenticate_mcp_server() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    // Press Enter on the server entry
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn selecting_non_oauth_server_is_noop() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "github",
        McpServerStatus::Connected { tool_count: 5 },
    )]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "should not emit command for non-OAuth server");
}

#[test]
fn provider_login_emits_authenticate() {
    let (mut app, mut command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("codex", "Codex", None)],
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    // Navigate to Provider Logins (menu: Theme, MCP Servers, Provider Logins)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Authenticate { method_id } => assert_eq!(method_id, "codex"),
        other => panic!("expected Authenticate, got: {other:?}"),
    }
}

#[test]
fn authenticate_complete_updates_correct_entry() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("a", "A", None), auth_method("b", "B", None)],
    );

    // Open provider logins and start auth for "a"
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate "a"

    // Simulate authenticate complete for method "a"
    app.on_acp_event(auth_complete("a"));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("logged in"), "should show logged in for 'a':\n{viewport}");
}

#[test]
fn authenticate_failed_resets_to_needs_login() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("x", "X", None)],
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate "x"

    app.on_acp_event(auth_failed("x"));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("needs login"), "should show needs login after failure:\n{viewport}");
}

#[test]
fn auth_methods_updated_replaces_provider_entries() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("old", "Old", None)],
    );

    // Open provider logins
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Replace auth methods
    app.on_acp_event(auth_methods_updated(vec![auth_method("new", "New", None)]));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("New"), "should show new provider:\n{viewport}");
    assert!(!viewport.contains("Old"), "should not show old provider:\n{viewport}");
}

#[test]
fn closing_settings_overlay_cancels_pending_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-1".to_string(),
                },
            },
            responder,
        });

        app.on_key(key(KeyCode::Esc));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, acp_utils::notifications::ElicitationAction::Cancel);
    });
}

#[test]
fn server_status_updated_while_pane_open_refreshes() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // Open MCP servers pane

    // Send update while pane is open
    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "a",
        McpServerStatus::Failed { error: "crash".to_string() },
    )]));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("crash"), "should show updated status:\n{viewport}");
}

#[test]
fn provider_login_pane_shows_all_statuses() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("needs", "NeedsLogin", None), auth_method("authd", "Authed", Some("authenticated"))],
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("NeedsLogin"), "should show NeedsLogin:\n{viewport}");
    assert!(viewport.contains("needs login"), "should show needs login status:\n{viewport}");
    assert!(viewport.contains("Authed"), "should show Authed:\n{viewport}");
    assert!(viewport.contains("logged in"), "should show logged in status:\n{viewport}");
}

#[test]
fn esc_from_server_status_returns_to_menu() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // Enter MCP Servers
    app.on_key(key(KeyCode::Esc)); // Back to menu

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("MCP Servers"), "should be back at menu:\n{viewport}");
}

#[test]
fn esc_from_provider_login_returns_to_menu() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        Vec::new(),
        vec![auth_method("x", "X", None)],
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Provider Logins"), "should be back at menu:\n{viewport}");
}

#[test]
fn server_status_summary_updates_in_menu() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("1 connected") || viewport.contains("1 failed"), "should show summary:\n{viewport}");
}

fn proxied_server_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_proxied(true)
}

fn proxied_oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_proxied(true).with_auth_capability(McpServerAuthCapability::OAuth)
}

#[test]
fn server_status_pane_groups_direct_and_proxied_with_headers() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Direct"), "should show Direct header:\n{viewport}");
    assert!(viewport.contains("Proxied"), "should show Proxied header:\n{viewport}");
    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("math"), "should show math:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
}

#[test]
fn server_status_pane_only_direct_renders_no_headers() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("slack", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Direct"), "should not show Direct header when all direct:\n{viewport}");
    assert!(!viewport.contains("Proxied"), "should not show Proxied header when no proxied:\n{viewport}");
}

#[test]
fn server_status_pane_only_proxied_shows_proxied_header() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Direct"), "should not show Direct header when all proxied:\n{viewport}");
    assert!(viewport.contains("Proxied"), "should show Proxied header:\n{viewport}");
}

#[test]
fn server_status_navigation_skips_headers_and_spacers() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // First server: github (Direct section, row index 1)
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "github is not OAuth, should be noop");

    // Move down once: should land on math (Proxied section, row index 4 - skipping Direct header)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "math is not OAuth, should be noop");

    // Move down once: should land on linear (Proxied section, row index 5)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }

    // Move up: should go back to math, not land on Proxied header or Spacer
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "math is not OAuth after wrap-up");

    // Move up again: should land back on github
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "github is not OAuth after wrap-up");
}

#[test]
fn proxied_oauth_server_sends_original_server_name() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![proxied_oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn connection_closed_cancels_settings_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-conn-closed".to_string(),
                },
            },
            responder,
        });

        app.on_acp_event(AcpEvent::ConnectionClosed);

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
    });
}

#[test]
fn new_session_created_cancels_settings_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-new-session".to_string(),
                },
            },
            responder,
        });

        app.on_acp_event(AcpEvent::NewSessionCreated { session_id: SessionId::new("new"), config_options: vec![] });

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
    });
}

#[test]
fn server_status_update_entries_preserves_selection_across_group_boundaries() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Navigate to linear (Proxied section, after header + spacer)
    app.on_key(key(KeyCode::Down));

    // Send update that flips the grouping - github becomes proxied too, linear stays OAuth
    app.on_acp_event(mcp_notification(vec![
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
        proxied_server_entry("github", McpServerStatus::Connected { tool_count: 5 }),
    ]));

    app.on_key(key(KeyCode::Enter));
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer for linear, got: {other:?}"),
    }
}

fn make_app_with_prompt_capabilities(
    prompt_capabilities: acp::PromptCapabilities,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options: Vec::new(),
        auth_methods: Vec::new(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

fn make_app_with_caps_and_config(
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options,
        auth_methods: Vec::new(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

fn make_failable_app_with_caps(
    prompt_capabilities: acp::PromptCapabilities,
) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options: vec![],
        auth_methods: vec![],
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, fail_signal, command_rx)
}

fn media_caps() -> acp::PromptCapabilities {
    acp::PromptCapabilities::new().image(true).audio(true)
}

fn model_select_option(
    value: &str,
    name: &str,
    supports_image: bool,
    supports_audio: bool,
) -> acp::SessionConfigSelectOption {
    acp::SessionConfigSelectOption::new(value.to_string(), name.to_string())
        .meta(SelectOptionMeta { reasoning_levels: vec![], supports_image, supports_audio }.into_meta())
}

fn create_temp_file(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, content).unwrap();
    p
}

fn image_model_config(current: &str, options: Vec<acp::SessionConfigSelectOption>) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        ConfigOptionId::Model.as_str().to_string(),
        "Model".to_string(),
        current.to_string(),
        options,
    )
    .category(acp::SessionConfigOptionCategory::Model)
}

fn grouped_model_config(current: &str, groups: Vec<acp::SessionConfigSelectGroup>) -> acp::SessionConfigOption {
    let mut option = acp::SessionConfigOption::select(
        ConfigOptionId::Model.as_str().to_string(),
        "Model".to_string(),
        current.to_string(),
        Vec::<acp::SessionConfigSelectOption>::new(),
    )
    .category(acp::SessionConfigOptionCategory::Model);
    if let acp::SessionConfigKind::Select(select) = &mut option.kind {
        select.options = acp::SessionConfigSelectOptions::Grouped(groups);
    }
    option
}

fn make_select_group(
    id: &str,
    name: &str,
    options: Vec<acp::SessionConfigSelectOption>,
) -> acp::SessionConfigSelectGroup {
    acp::SessionConfigSelectGroup::new(acp::SessionConfigGroupId::new(id.to_string()), name.to_string(), options)
}

#[test]
fn paste_image_path_adds_pending_media() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.on_paste(img.to_str().unwrap());

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "photo.png");
    assert!(app.composer().text().is_empty());
}

#[test]
fn paste_audio_path_adds_pending_media() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav");

    app.on_paste(audio.to_str().unwrap());

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "note.wav");
}

#[test]
fn paste_ordinary_text_inserts_as_text() {
    let (mut app, _command_rx) = make_app();

    app.on_paste("hello world");

    assert!(app.composer().pending_media().is_empty());
    assert_eq!(app.composer().text(), "hello world");
}

#[test]
fn paste_non_media_file_falls_back_to_text() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let txt = create_temp_file(&tmp, "readme.txt", b"hello");

    app.on_paste(txt.to_str().unwrap());

    assert!(app.composer().pending_media().is_empty());
    assert!(!app.composer().text().is_empty());
}

#[test]
fn paste_multiple_dropped_files_adds_all() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "a.png", b"img");
    let audio = create_temp_file(&tmp, "b.wav", b"audio");
    let input = format!("{}\n{}", img.display(), audio.display());

    app.on_paste(&input);

    assert_eq!(app.composer().pending_media().len(), 2);
}

#[test]
fn duplicate_dropped_media_not_added_twice() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");
    let path_str = img.to_str().unwrap().to_string();

    app.on_paste(&path_str);
    app.on_paste(&path_str);

    assert_eq!(app.composer().pending_media().len(), 1);
}

#[test]
fn media_only_submit_sends_with_content_blocks() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert!(text.is_empty(), "media-only send should have empty text");
            assert!(content.is_some(), "media-only send should have content blocks");
            assert!(!content.unwrap().is_empty());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_with_text_and_media_merges_both() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this");
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "describe this");
            assert!(content.is_some());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_clears_pending_media() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    command_rx.try_recv().unwrap();

    assert!(app.composer().pending_media().is_empty());
    assert!(app.composer().text().is_empty());
}

#[test]
fn backspace_on_empty_composer_removes_last_dropped_media() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img1 = create_temp_file(&tmp, "a.png", b"a");
    let img2 = create_temp_file(&tmp, "b.png", b"b");

    app.on_paste(img1.to_str().unwrap());
    app.on_paste(img2.to_str().unwrap());
    assert_eq!(app.composer().pending_media().len(), 2);

    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "a.png");

    app.on_key(key(KeyCode::Backspace));
    assert!(app.composer().pending_media().is_empty());
}

#[test]
fn backspace_does_not_remove_media_when_text_present() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "x");
    app.on_key(key(KeyCode::Backspace));

    assert_eq!(app.composer().pending_media().len(), 1);
    assert!(app.composer().text().is_empty());
}

#[test]
fn attachment_chips_render_in_layout() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());

    let layout = app.composer().layout(80, &Theme::default());
    let text: String = layout
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("attached image: photo.png"));
}

#[test]
fn agent_rejects_image_when_capability_missing() {
    let caps = acp::PromptCapabilities::new().image(false).audio(true);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    assert!(!app.waiting_for_response());

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("does not support image")));
}

#[test]
fn agent_rejects_audio_when_capability_missing() {
    let caps = acp::PromptCapabilities::new().image(true).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    assert!(!app.waiting_for_response());
}

#[test]
fn selected_model_rejects_image() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-vision",
        vec![model_select_option("gpt:no-vision", "GPT No Vision", false, false)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support image")));
}

#[test]
fn missing_model_metadata_rejects_media() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config =
        vec![image_model_config("unknown-model", vec![model_select_option("known-model", "Known", true, true)])];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("missing prompt capability metadata")));
}

#[test]
fn supported_media_sends_blocks() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "claude:vision",
        vec![model_select_option("claude:vision", "Claude Vision", true, true)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn sync_prompt_failure_resets_busy_state() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_caps(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));

    assert!(!app.waiting_for_response(), "sync prompt failure should reset busy state");
    assert!(command_rx.try_recv().is_err(), "no prompt should be sent");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")));
}

#[test]
fn text_only_submit_unaffected_by_media_capability_check() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);

    submit_prompt(&mut app, "hello");

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "hello");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_is_blocked_when_composer_empty_without_media() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err());
}

#[test]
fn clear_command_also_clears_pending_media() {
    let (mut app, mut command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    assert!(app.composer().pending_media().is_empty());
    assert!(app.composer().text().is_empty());
}

#[test]
fn paste_with_file_uri_parses_correctly() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "image.png", b"img");
    let uri = format!("file://{}", img.display());

    app.on_paste(&uri);

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "image.png");
}

#[test]
fn paste_with_percent_decoded_file_uri() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "my image.png", b"png");
    let unencoded_path = img.to_str().unwrap();
    let encoded_path = unencoded_path.replace(' ', "%20");
    let uri = format!("file://{encoded_path}");

    app.on_paste(&uri);

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "my image.png");
}

#[test]
fn selected_model_rejects_audio() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-audio",
        vec![model_select_option("gpt:no-audio", "GPT No Audio", true, false)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support audio")));
}

#[test]
fn selected_model_rejects_image_grouped() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![make_select_group(
        "g1",
        "Group 1",
        vec![model_select_option("grouped:no-vision", "No Vision", false, true)],
    )];
    let config = vec![grouped_model_config("grouped:no-vision", groups)];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support image")));
}

#[test]
fn selected_model_rejects_audio_grouped() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![make_select_group(
        "g1",
        "Group 1",
        vec![model_select_option("grouped:no-audio", "No Audio", true, false)],
    )];
    let config = vec![grouped_model_config("grouped:no-audio", groups)];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support audio")));
}

#[test]
fn comma_separated_multi_model_rejects_image() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![
        make_select_group(
            "g1",
            "Vision Models",
            vec![model_select_option("claude:sonnet", "Claude Sonnet", true, true)],
        ),
        make_select_group(
            "g2",
            "Text Models",
            vec![model_select_option("gpt:text-only", "GPT Text Only", false, false)],
        ),
    ];
    let config = vec![grouped_model_config("claude:sonnet,gpt:text-only", groups)];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked — multi-select includes unsupported model");
}

#[test]
fn comma_separated_multi_model_sends_when_all_support_media() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![
        make_select_group(
            "g1",
            "Vision Models",
            vec![model_select_option("claude:sonnet", "Claude Sonnet", true, true)],
        ),
        make_select_group("g2", "Reasoning", vec![model_select_option("deepseek:r1", "DeepSeek R1", true, true)]),
    ];
    let config = vec![grouped_model_config("claude:sonnet,deepseek:r1", groups)];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn rejection_preserves_text_and_placeholders_in_transcript() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this image");
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg == "describe this image"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("does not support image")), "error message shown");
}

#[test]
fn sync_failure_preserves_text_and_placeholders_in_transcript() {
    let caps = media_caps();
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_caps(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this");
    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));

    assert!(!app.waiting_for_response(), "sync failure should reset busy state");
    assert!(command_rx.try_recv().is_err(), "no prompt should be sent");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg == "describe this"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")), "error message shown");
}

// ── Workspace move tests ──

#[test]
fn workspace_move_command_hidden_without_capability() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/clear"), "{viewport}");
    assert!(!viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_visible_with_capability() {
    let (mut app, _command_rx) = make_app_with_workspace_move();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_prompt_in_flight() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    submit_prompt(&mut app, "hello");
    let _ = command_rx.try_recv().unwrap();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));

    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Cannot move") && l.contains("workspace")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("prompt is running")), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_already_listing() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Listing);

    let list_cmd = command_rx.try_recv().unwrap();
    assert!(matches!(list_cmd, PromptCommand::ListWorkspaces(_)));

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Listing);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("another move is in progress"), "{viewport}");
}

#[test]
fn workspace_list_synchronous_failure_resets_state() {
    let (mut app, fail_signal, _command_rx) = make_failable_app_with_workspace_move();

    fail_signal.store(true, Ordering::SeqCst);
    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));

    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces"), "{viewport}");
}

#[test]
fn workspace_list_failed_event_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Listing);
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspace_list_failed("network error"));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces: network error"), "{viewport}");
}

#[test]
fn workspace_picker_opens_with_existing_workspaces() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Listing);
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
        workspace_entry("/tmp/sandbox", false),
    ]));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/home/user/code/other"), "{viewport}");
    assert!(viewport.contains("/tmp/sandbox"), "{viewport}");
    assert!(!viewport.contains("/home/user/code/current"), "current workspace should be excluded:\n{viewport}");
    assert!(viewport.contains("Create new workspace"), "{viewport}");
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("No other workspaces available"), "{viewport}");
}

#[test]
fn workspace_picker_esc_closes_and_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);
    assert!(!app.has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Moving);
    assert!(!app.has_modal());

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test-session");
            match params.target {
                acp_utils::notifications::WorkspaceMoveTarget::Existing { path } => {
                    assert_eq!(path, std::path::PathBuf::from("/home/user/code/other"));
                }
                other @ acp_utils::notifications::WorkspaceMoveTarget::New { .. } => {
                    panic!("expected Existing, got {other:?}")
                }
            }
        }
        other => panic!("expected MoveWorkspace, got {other:?}"),
    }
}

#[test]
fn workspace_picker_enter_selects_create_new_and_shows_naming_mode() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Create new workspace"), "{viewport}");

    app.on_key(key(KeyCode::Enter));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("New workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_esc_returns_to_list_mode() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    app.on_key(key(KeyCode::Enter));
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("New workspace"), "{viewport}");

    app.on_key(key(KeyCode::Esc));
    assert!(app.has_modal());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("Create new workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_enter_with_name_emits_move_target() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "my-new-workspace");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Moving);
    assert!(!app.has_modal());

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test-session");
            match params.target {
                acp_utils::notifications::WorkspaceMoveTarget::New { name } => {
                    assert_eq!(name, "my-new-workspace");
                }
                other @ acp_utils::notifications::WorkspaceMoveTarget::Existing { .. } => {
                    panic!("expected New, got {other:?}")
                }
            }
        }
        other => panic!("expected MoveWorkspace, got {other:?}"),
    }
}

#[test]
fn workspace_picker_filtering_hides_non_matching() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
        workspace_entry("/tmp/test", false),
    ]));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("project-a"), "{viewport}");
    assert!(viewport.contains("/tmp/test"), "{viewport}");

    // Use a query that only matches one entry
    app.on_key(key(KeyCode::Char('j')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("project-a"), "{viewport2}");
    assert!(!viewport2.contains("/tmp/test"), "{viewport2}");
    assert!(!viewport2.contains("Create new"), "{viewport2}");
}

#[test]
fn workspace_move_success_updates_cwd_and_reloads_session() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Moving);

    let cmd = command_rx.try_recv().unwrap();
    assert!(matches!(cmd, PromptCommand::MoveWorkspace { .. }));

    app.on_acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::LoadingSession);

    let load_cmd = command_rx.try_recv().unwrap();
    match load_cmd {
        PromptCommand::LoadSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(cwd, std::path::Path::new("/home/user/code/other"));
        }
        other => panic!("expected LoadSession, got {other:?}"),
    }

    app.on_acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);
}

#[test]
fn workspace_move_success_buffers_and_replays_session_updates() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspace_moved("/home/user/code/other"));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_update_for("test-session", user_message_chunk("buffered-message")));

    app.on_acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("buffered-message")), "{viewport}");
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Moved to /home/user/code/other"), "{viewport}");
}

#[test]
fn workspace_move_load_session_failure_recovers() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    fail_signal.store(true, Ordering::SeqCst);
    app.on_acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Failed to reload session")), "{viewport}");
}

#[test]
fn workspace_move_failed_event_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Moving);

    app.on_acp_event(workspace_move_failed("permission denied"));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Workspace move failed")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("permission denied")), "{viewport}");
}

#[test]
fn workspace_move_synchronous_error_resets_state() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    fail_signal.store(true, Ordering::SeqCst);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to move workspace"), "{viewport}");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
    ]));

    let mut terminal = make_terminal_with_width(40);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("project-a"), "{viewport}");
}

#[test]
fn workspace_move_picker_closes_when_connection_closes() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_modal());
    assert_eq!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Idle);
}

fn make_failable_app_with_workspace_move() -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
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

fn make_app_with_prompt_search() -> (App, UnboundedReceiver<PromptCommand>) {
    let session_capabilities = acp::SessionCapabilities::new().meta(Some(
        AetherCapabilities { prompt_search: true, session_preview: false, workspace_move: false }.to_meta(),
    ));
    make_app_with_metadata(std::path::PathBuf::from("."), session_capabilities, Vec::new(), Vec::new())
}

fn prompt_search_result(prompt: &str, start: usize, end: usize) -> acp_utils::notifications::PromptSearchResult {
    prompt_search_result_with_cwd(prompt, start, end, std::path::PathBuf::from("/tmp/repo"))
}

fn prompt_search_result_with_cwd(
    prompt: &str,
    start: usize,
    end: usize,
    cwd: std::path::PathBuf,
) -> acp_utils::notifications::PromptSearchResult {
    acp_utils::notifications::PromptSearchResult {
        session_id: "s1".to_string(),
        cwd,
        session_created_at: "2026-05-17T00:00:00Z".to_string(),
        prompt: prompt.to_string(),
        match_start: start,
        match_end: end,
    }
}

fn prompt_search_response(
    query: &str,
    results: Vec<acp_utils::notifications::PromptSearchResult>,
) -> acp_utils::notifications::PromptSearchResponse {
    prompt_search_response_gen(query, results, 1)
}

fn prompt_search_response_gen(
    query: &str,
    results: Vec<acp_utils::notifications::PromptSearchResult>,
    generation: u64,
) -> acp_utils::notifications::PromptSearchResponse {
    acp_utils::notifications::PromptSearchResponse {
        query: query.to_string(),
        results,
        truncated: false,
        search_generation: generation,
    }
}

#[test]
fn ctrl_r_is_noop_when_prompt_search_capability_is_missing() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    assert_eq!(app.composer().text(), "draft");
}

#[test]
fn ctrl_r_opens_prompt_search_when_capability_is_enabled() {
    let (mut app, _command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    assert!(app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "draft");
}

#[test]
fn prompt_search_shows_loading_state_after_query() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));

    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::SearchPrompts(params) if params.query == "h"),
        "expected SearchPrompts with query 'h', got {cmd:?}"
    );

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("history search: h"), "viewport:\n{viewport}");
    assert!(viewport.contains("searching"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_empty_query_renders_instruction() {
    let (mut app, _command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("history search:"), "viewport:\n{viewport}");
    assert!(viewport.contains("type to search prompt history"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_shows_results_after_response() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello world", 0, 1)],
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("hello world"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_no_results_shows_no_matches() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    type_text(&mut app, "zzz");
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen("zzz", vec![], 3)));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("no matching prompts"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_shows_error_on_failure() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchFailed {
        query: "h".to_string(),
        search_generation: 1,
        error: "connection refused".to_string(),
    });

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("error: connection refused"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_enter_confirms_and_inserts_result() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello world", 0, 5)],
    )));

    app.on_key(key(KeyCode::Enter));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "hello world");
}

#[test]
fn prompt_search_enter_without_selection_restores_draft() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    type_text(&mut app, "zzz");
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen("zzz", vec![], 3)));

    app.on_key(key(KeyCode::Enter));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "draft");
}

#[test]
fn prompt_search_escape_restores_draft() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "original draft");
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello world", 0, 5)],
    )));

    app.on_key(key(KeyCode::Esc));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "original draft");
}

#[test]
fn prompt_search_escape_restores_multiline_draft() {
    let (mut app, _command_rx) = make_app_with_prompt_search();
    app.on_key(key(KeyCode::Char('l')));
    app.on_key(key(KeyCode::Char('i')));
    app.on_key(key(KeyCode::Char('n')));
    app.on_key(key(KeyCode::Char('e')));
    app.on_key(key(KeyCode::Char('1')));
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.on_key(key(KeyCode::Char('l')));
    app.on_key(key(KeyCode::Char('i')));
    app.on_key(key(KeyCode::Char('n')));
    app.on_key(key(KeyCode::Char('e')));
    app.on_key(key(KeyCode::Char('2')));

    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Esc));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "line1\nline2");
}

#[test]
fn prompt_search_up_and_down_change_selection() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello", 0, 1), prompt_search_result("hey", 0, 1)],
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("hello"), "viewport:\n{viewport}");

    app.on_key(key(KeyCode::Down));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("hey"), "viewport:\n{viewport}");

    app.on_key(key(KeyCode::Up));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("hello"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_stale_response_is_ignored() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('e')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "he",
        vec![prompt_search_result("hello", 0, 2)],
        2,
    )));

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "h",
        vec![prompt_search_result("STALE", 0, 1)],
        1,
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("hello"), "should show current result:\n{viewport}");
    assert!(!viewport.contains("STALE"), "should not show stale result:\n{viewport}");
}

#[test]
fn prompt_search_prefills_selected_result_with_cursor_at_match() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('q')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "q",
        vec![prompt_search_result("the quick brown fox", 4, 9)],
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("the quick brown fox"), "viewport:\n{viewport}");

    let (row, col) = app.composer().cursor_position();
    assert_eq!((row, col), (0, 9), "cursor should be at match end position 9");
}

#[test]
fn prompt_search_paste_sanitizes_query() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));

    app.on_paste("hello\nworld");

    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::SearchPrompts(params) if params.query == "helloworld"),
        "expected sanitized query 'helloworld', got {cmd:?}"
    );
}

#[test]
fn prompt_search_backspace_to_empty_restores_draft_but_keeps_picker_open() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello", 0, 1)],
    )));

    app.on_key(key(KeyCode::Backspace));

    assert!(app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "draft");

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("type to search prompt history"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_ctrl_r_does_not_steal_from_modal() {
    let (mut app, _command_rx) = make_app_with_prompt_search();

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (cx, mut peer) = test_connection().await;
        let (responder, _response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
    });

    assert!(app.has_modal());
    app.on_key(ctrl('r'));
    assert!(!app.composer().has_prompt_search());
}

#[test]
fn prompt_search_ctrl_r_does_not_open_during_composer_overlay() {
    let (mut app, _command_rx) = make_app_with_prompt_search();
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    app.on_key(ctrl('r'));
    assert!(!app.composer().has_prompt_search());
    assert!(app.composer().has_overlay());
}

#[test]
fn prompt_search_unicode_query_is_accepted() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('ñ')));

    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::SearchPrompts(params) if params.query == "ñ"),
        "expected unicode query 'ñ', got {cmd:?}"
    );

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("history search: ñ"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_rows_truncate_prompt_and_show_cwd_basename() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    type_text(&mut app, "quick");
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "quick",
        vec![prompt_search_result_with_cwd(
            "the quick brown fox jumps over the lazy dog",
            4,
            9,
            std::path::PathBuf::from("/some/deeply/nested/project/repo-name"),
        )],
        5,
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("…"), "should have ellipsis in truncated prompt:\n{viewport}");
    assert!(viewport.contains("repo-name"), "should show cwd basename:\n{viewport}");
}

#[test]
fn prompt_search_query_editing_triggers_multiple_searches() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('a')));
    app.on_key(key(KeyCode::Char('b')));

    let cmd1 = command_rx.try_recv().unwrap();
    let cmd2 = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd1, PromptCommand::SearchPrompts(params) if params.query == "a"),
        "first search should be for 'a', got {cmd1:?}"
    );
    assert!(
        matches!(&cmd2, PromptCommand::SearchPrompts(params) if params.query == "ab"),
        "second search should be for 'ab', got {cmd2:?}"
    );
}

// ── Prompt search regression tests (review findings) ──

#[test]
fn prompt_search_enter_preserves_cursor_at_match_end() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    type_text(&mut app, "draft");
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('q')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "q",
        vec![prompt_search_result("the quick brown fox", 4, 9)],
    )));

    app.on_key(key(KeyCode::Enter));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "the quick brown fox");
    let (row, col) = app.composer().cursor_position();
    assert_eq!((row, col), (0, 9), "cursor must be at match end (9), not end of prompt");
}

#[test]
fn prompt_search_enter_preserves_cursor_after_manual_navigation() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('h')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello", 0, 1), prompt_search_result("hi there", 0, 1)],
    )));

    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    assert!(!app.composer().has_prompt_search());
    assert_eq!(app.composer().text(), "hi there");
    let (row, col) = app.composer().cursor_position();
    assert_eq!((row, col), (0, 1), "cursor must be at match end (1) for 'hi there'");
}

#[test]
fn prompt_search_identical_repeated_query_uses_generation_not_just_string() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));

    // First search for "xy"
    app.on_key(key(KeyCode::Char('x')));
    app.on_key(key(KeyCode::Char('y')));
    let _ = command_rx.try_recv().unwrap();
    let _ = command_rx.try_recv().unwrap();

    // Backspace twice to get empty query (draft restored)
    app.on_key(key(KeyCode::Backspace));
    app.on_key(key(KeyCode::Backspace));

    // Type "xy" again — same query string but generation is now higher
    app.on_key(key(KeyCode::Char('x')));
    app.on_key(key(KeyCode::Char('y')));
    let _ = command_rx.try_recv().unwrap();
    let _ = command_rx.try_recv().unwrap();

    // Stale response from first "xy" with generation=2 should be ignored
    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("STALE_FIRST", 0, 2)],
        2,
    )));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("STALE_FIRST"), "stale response from first 'xy' must be ignored:\n{viewport}");
    assert!(viewport.contains("searching"), "second 'xy' should still be loading:\n{viewport}");

    // Fresh response from second "xy" with generation=5 should be accepted
    // (gen: 0→x=1, xy=2, backspace=3, backspace(empty)=3, x=4, xy=5)
    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("FRESH_SECOND", 0, 2)],
        5,
    )));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("FRESH_SECOND"), "fresh response from second 'xy' must be shown:\n{viewport}");
}

#[test]
fn prompt_search_send_failure_is_visible_in_picker() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_prompt_search();
    app.on_key(ctrl('r'));

    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Char('h')));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("search failed"), "send failure must be visible in picker:\n{viewport}");

    assert!(app.composer().has_prompt_search(), "picker must remain open");
    assert!(!app.exit_requested(), "app must remain interactive");
}

#[test]
fn prompt_search_stale_failure_is_accepted_for_current_query() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('x')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchFailed {
        query: "x".to_string(),
        search_generation: 1,
        error: "server error".to_string(),
    });

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("server error"), "failure must be visible:\n{viewport}");

    assert!(app.composer().has_prompt_search(), "picker must remain open after failure");
}

#[test]
fn prompt_search_stale_failure_must_not_overwrite_newer_success() {
    let (mut app, mut command_rx) = make_app_with_prompt_search();
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('x')));
    let _ = command_rx.try_recv().unwrap();

    app.on_key(key(KeyCode::Char('y')));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("fresh result for xy", 0, 2)],
        2,
    )));

    app.on_acp_event(AcpEvent::PromptSearchFailed {
        query: "x".to_string(),
        search_generation: 1,
        error: "stale error".to_string(),
    });

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("fresh result for xy"), "newer success results must survive stale failure:\n{viewport}");
    assert!(!viewport.contains("stale error"), "stale failure must be ignored:\n{viewport}");
}

// ── Settings overlay integration tests ──

#[test]
fn settings_builtin_opens_overlay_and_clears_composer() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    // Composer should be cleared
    assert!(app.composer().text().is_empty());
    // Should not emit a Prompt
    assert!(command_rx.try_recv().is_err());
    // Overlay should be open (has_modal returns true)
    assert!(app.has_modal());
}

#[test]
fn settings_builtin_is_listed_in_command_picker() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/settings"), "built-in /settings should be in command picker:\n{viewport}");
}

#[test]
fn settings_esc_closes_overlay() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_key(key(KeyCode::Esc));
    assert!(!app.has_modal());
}

#[test]
fn settings_overlay_renders_on_terminal() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o"), select_option("mode", "code")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings overlay should show model entry:\n{viewport}"
    );
    assert!(
        viewport.contains("mode") || viewport.contains("code"),
        "settings overlay should show mode entry:\n{viewport}"
    );
}

#[test]
fn settings_over_renders_with_no_config_options() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("no settings options") || viewport.contains("Configuration"),
        "empty settings should show placeholder:\n{viewport}"
    );
}

#[test]
fn settings_overlay_still_valid_after_scrollback() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o"), select_option("mode", "code")],
        Vec::new(),
    );

    // Fill transcript with content to push scrollback
    for i in 0..30 {
        app.on_acp_event(text_chunk(&format!("line {i}")));
    }
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Now open settings
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Configuration"), "settings overlay should render after scrollback:\n{viewport}");
}

#[test]
fn settings_overlay_renders_at_narrow_width() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal_with_width(30);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings should render at 30 cols:\n{viewport}"
    );
}

#[test]
fn settings_overlay_renders_at_short_height() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o"), select_option("mode", "code")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal_with_width(40);
    terminal.backend_mut().resize(40, 8);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("too small"),
        "settings should handle short terminal:\n{viewport}"
    );
}

#[test]
fn settings_selecting_option_emits_config_option() {
    let (mut app, mut command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "gpt-4o",
            vec![
                acp::SessionConfigSelectOption::new("gpt-4o", "GPT-4o"),
                acp::SessionConfigSelectOption::new("claude", "Claude"),
            ],
        )],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Down past the Theme entry, Enter to open picker, Down to second option, Enter to confirm
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Should have emitted a set_config_option command
    let cmd = command_rx.try_recv().expect("expected set_config_option");
    match cmd {
        PromptCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert_eq!(value, "claude");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn settings_multi_select_opens_model_selector() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Down past the Theme entry, then Enter should open model selector since multi_select is true
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Model search"), "should show model selector:\n{viewport}");
}

#[test]
fn settings_multi_select_toggle_and_confirm() {
    let (mut app, mut command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme entry
    app.on_key(key(KeyCode::Enter)); // Open model selector

    app.on_key(key(KeyCode::Enter)); // Toggle first model
    app.on_key(key(KeyCode::Esc)); // Confirm and close

    let cmd = command_rx.try_recv().expect("expected set_config_option");
    match cmd {
        PromptCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert!(value.contains("anthropic:opus"), "value: {value}");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn config_option_update_refreshes_settings_overlay() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Simulate config update from server
    app.on_acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
        select_option("mode", "code"),
    ]))));

    // Overlay should still be open with updated options
    assert!(app.has_modal());
    let options = app.config_options();
    assert_eq!(options.len(), 2);
}

#[test]
fn config_option_update_failed_shows_in_transcript() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "invalid model".to_string() });

    // Overlay should still be open
    assert!(app.has_modal());

    // Error should be in transcript
    let items = app.drain_finalized();
    let has_error = items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("invalid model")));
    assert!(has_error, "expected transcript error, got {items:?}");
}

#[test]
fn connection_closed_clears_settings_overlay() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_modal());
    assert!(app.exit_requested());
}

#[test]
fn new_session_clears_settings_overlay() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // New session created should close settings overlay
    app.on_acp_event(new_session_created("new-id", Vec::new()));
    assert!(!app.has_modal());

    // Should have consumed the new session event
    let _ = command_rx.try_recv().ok();
}

#[test]
fn settings_composer_capture_prevents_normal_input() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    let composer_text_before = app.composer().text().to_string();

    // Typing while settings overlay is open should not modify composer
    for c in "hello".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.composer().text(), composer_text_before);
}

#[test]
fn settings_theme_entry_is_injected_first() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme:"), "Theme entry should render first:\n{viewport}");
}

#[test]
fn settings_theme_picker_opens_and_shows_default() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker (first entry)

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Default"), "Theme picker should show Default option:\n{viewport}");
    assert!(viewport.contains("Theme"), "Theme picker should have Theme header:\n{viewport}");
}

#[test]
fn settings_theme_selection_returns_to_menu() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker
    app.on_key(key(KeyCode::Enter)); // Confirm default

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme: Default"), "Should return to menu with Default selected:\n{viewport}");
}

#[test]
fn settings_theme_empty_file_list_shows_only_default() {
    let (mut app, _command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), Vec::new(), Vec::new());

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Default"), "Should show Default theme option:\n{viewport}");
}

// ── Regression tests for review findings ──

#[test]
fn theme_entry_preserved_after_config_option_update() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // ConfigOptionUpdate arrives — Theme entry must still be first
    app.on_acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
    ]))));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme:"), "Theme entry must survive ConfigOptionUpdate:\n{viewport}");
}

#[test]
fn theme_selection_keeps_overlay_open_and_refreshes_display() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![select_option("model", "gpt-4o")],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker
    app.on_key(key(KeyCode::Enter)); // Confirm default theme

    // Overlay must stay open
    assert!(app.has_modal(), "Overlay must stay open after theme selection");

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme: Default"), "Theme should show Default after selection:\n{viewport}");
}

#[test]
fn model_selector_provider_heading_does_not_skip_rows() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o"),
                    acp::SessionConfigSelectOption::new("openai:gpt-3.5", "OpenAI / GPT-3.5"),
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme
    app.on_key(key(KeyCode::Enter)); // Open model selector

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    // All four models must appear — headings should not consume model rows
    assert!(viewport.contains("GPT-4o"), "GPT-4o should be visible:\n{viewport}");
    assert!(viewport.contains("GPT-3.5"), "GPT-3.5 should be visible:\n{viewport}");
    assert!(viewport.contains("Opus"), "Opus should be visible:\n{viewport}");
    assert!(viewport.contains("Sonnet"), "Sonnet should be visible:\n{viewport}");
}

#[test]
fn model_selector_focused_item_visible_with_provider_headings() {
    let (mut app, _command_rx) = make_app_with_metadata(
        std::path::PathBuf::from("."),
        acp::SessionCapabilities::new(),
        vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o"),
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                    acp::SessionConfigSelectOption::new("google:gemini", "Google / Gemini"),
                    acp::SessionConfigSelectOption::new("google:palm", "Google / PaLM"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }],
        Vec::new(),
    );

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme
    app.on_key(key(KeyCode::Enter)); // Open model selector

    // Move down through all items — last item should be visible
    for _ in 0..4 {
        app.on_key(key(KeyCode::Down));
    }

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("PaLM"), "Focused last item with headings should be visible:\n{viewport}");
}

// ── Composer framing ────────────────────────────────────────────

#[test]
fn composer_empty_renders_top_and_bottom_rules() {
    let composer = Composer::new();
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.lines.len(), 3, "empty composer: top rule, prompt, bottom rule");
    assert!(line_text(&layout.lines[0]).chars().all(|c| c == '─'), "top rule");
    assert_eq!(line_text(&layout.lines[0]).chars().count(), 80, "top rule spans full width");
    assert!(line_text(&layout.lines[2]).chars().all(|c| c == '─'), "bottom rule");
}

#[test]
fn composer_single_line_renders_top_and_bottom_rules() {
    let mut composer = Composer::new();
    composer.insert_str("hello");
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.lines.len(), 3, "single line: top rule, prompt line, bottom rule");
    assert!(line_text(&layout.lines[0]).chars().all(|c| c == '─'));
    assert!(line_text(&layout.lines[1]).contains("> hello"));
    assert!(line_text(&layout.lines[2]).chars().all(|c| c == '─'));
}

#[test]
fn composer_wrapped_renders_top_and_bottom_rules() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefghijkl");
    let layout = composer.layout(8, &Theme::default());

    assert_eq!(layout.lines.len(), 4, "wrapped: top rule, 2 content rows, bottom rule");
    assert!(line_text(&layout.lines[0]).chars().all(|c| c == '─'));
    assert!(line_text(layout.lines.last().unwrap()).chars().all(|c| c == '─'));
}

#[test]
fn composer_hard_newline_renders_top_and_bottom_rules() {
    let mut composer = Composer::new();
    composer.insert_str("one\ntwo");
    let layout = composer.layout(80, &Theme::default());

    let line_texts: Vec<String> = layout.lines.iter().map(line_text).collect();
    assert_eq!(line_texts, vec!["─".repeat(80), "> one".to_owned(), "  two".to_owned(), "─".repeat(80)]);
}

// ── Composer cursor ──────────────────────────────────────────────

#[test]
fn cursor_at_prompt_start() {
    let composer = Composer::new();
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.cursor.x, 2);
    assert_eq!(layout.cursor.y, 1, "cursor row is 1 (below top rule)");
}

#[test]
fn cursor_after_text() {
    let mut composer = Composer::new();
    composer.insert_str("abc");
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.cursor.x, 5, "2 (prefix) + 3 (abc)");
    assert_eq!(layout.cursor.y, 1);
}

#[test]
fn cursor_after_unicode_and_wide_chars() {
    let mut composer = Composer::new();
    composer.insert_str("a🎉界");
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.cursor.x, 7, "2 (prefix) + 1 (a) + 2 (🎉) + 2 (界)");
    assert_eq!(layout.cursor.y, 1);
}

#[test]
fn cursor_after_whitespace_wrap() {
    let mut composer = Composer::new();
    composer.insert_str("hello world");
    composer.move_left();
    composer.move_left();
    composer.move_left();
    composer.move_left();
    composer.move_left();
    let layout = composer.layout(9, &Theme::default());

    assert_eq!(layout.cursor.y, 1, "cursor on first wrapped line (byte 6 in chunk 'hello w')");
    assert_eq!(layout.cursor.x, 8);
}

#[test]
fn cursor_after_multiple_mentions() {
    let mut composer = Composer::new();
    composer.insert_str("@main.rs @lib.rs");
    let layout = composer.layout(80, &Theme::default());

    assert_eq!(layout.cursor.x, 18);
    assert_eq!(layout.cursor.y, 1);
}

#[test]
fn cursor_preserved_after_resize_reflow() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefgh");
    composer.move_left();
    composer.move_left();

    let wide = composer.layout(80, &Theme::default());
    let narrow = composer.layout(6, &Theme::default());

    assert_eq!(wide.lines.len(), 3, "wide: top rule, prompt, bottom rule");
    assert_eq!(narrow.lines.len(), 4, "narrow: top rule, 2 content rows, bottom rule");
    assert_eq!(wide.cursor.y, 1);
    assert_eq!(narrow.cursor.y, 2);
    assert_eq!(wide.cursor.x, 8);
    assert_eq!(narrow.cursor.x, 4);
}

// ── Overlays and tiny terminals ──────────────────────────────────

#[test]
fn tiny_terminal_does_not_overwrite_status_line() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    type_text(&mut app, "hello");

    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("aether"), "status line (agent name) still visible");
    assert!(viewport.contains("> hello"), "composer still visible");
}

#[test]
fn overlay_does_not_duplicate_border_rows() {
    let mut composer = Composer::new();
    composer.insert_char('/');
    composer.open_command_picker(vec![CommandEntry {
        name: "test".to_string(),
        description: "A test command".to_string(),
        has_input: false,
        hint: None,
        builtin: false,
    }]);

    let layout = composer.layout(80, &Theme::default());

    // framing + overlay should not duplicate rules
    assert!(line_text(&layout.lines[0]).chars().all(|c| c == '─'), "top rule present");
    assert!(line_text(layout.lines.last().unwrap()).chars().all(|c| c == '─'), "bottom rule present");
}

// ── Paste sanitization ───────────────────────────────────────────

#[test]
fn paste_strips_control_characters() {
    let mut composer = Composer::new();
    composer.insert_paste("abc\x01def\x02ghi");
    assert_eq!(composer.text(), "abcdefghi");
}

#[test]
fn paste_preserves_newlines() {
    let mut composer = Composer::new();
    composer.insert_paste("line one\nline two");
    assert_eq!(composer.text(), "line one\nline two");
}

#[test]
fn paste_preserves_tabs() {
    let mut composer = Composer::new();
    composer.insert_paste("col1\tcol2");
    assert_eq!(composer.text(), "col1\tcol2");
}

#[test]
fn paste_preserves_unicode() {
    let mut composer = Composer::new();
    composer.insert_paste("héllo 🎉 wörld");
    assert_eq!(composer.text(), "héllo 🎉 wörld");
}

#[test]
fn paste_strips_carriage_return_and_other_c0_controls() {
    let mut composer = Composer::new();
    composer.insert_paste("abc\r\x08\x7fdef");
    assert_eq!(composer.text(), "abcdef");
}

// ── Ctrl-A / Ctrl-E ──────────────────────────────────────────────

#[test]
fn ctrl_a_moves_to_line_start() {
    let mut composer = Composer::new();
    composer.insert_str("hello world");
    composer.move_left();
    composer.move_left();
    composer.move_line_start();
    assert_eq!(composer.cursor_position(), (0, 0));
}

#[test]
fn ctrl_e_moves_to_line_end() {
    let mut composer = Composer::new();
    composer.insert_str("hello world");
    composer.move_line_start();
    composer.move_line_end();
    assert_eq!(composer.cursor_position(), (0, 11));
}

#[test]
fn ctrl_a_stays_within_logical_line_for_multiline() {
    let mut composer = Composer::new();
    composer.insert_str("first line\nsecond line");
    // cursor at end of "second line"
    composer.move_line_start();
    assert_eq!(composer.cursor_position(), (1, 0), "cursor at start of second line");
}

#[test]
fn ctrl_e_stays_within_logical_line_for_multiline() {
    let mut composer = Composer::new();
    composer.insert_str("first line\nsecond line");
    composer.move_line_start();
    composer.move_line_start();
    composer.move_line_end();
    assert_eq!(composer.cursor_position(), (1, 11), "cursor stays at end of second line");
}

#[test]
fn app_routes_ctrl_a_to_move_line_start() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "hello");
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(ctrl('a'));
    assert_eq!(app.composer().cursor_position(), (0, 0));
}

#[test]
fn app_routes_ctrl_e_to_move_line_end() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "hello");
    app.on_key(key(KeyCode::Home));
    app.on_key(ctrl('e'));
    assert_eq!(app.composer().cursor_position(), (0, 5));
}

// ── Hard newline overlay closure ─────────────────────────────────

#[test]
fn shift_enter_closes_command_overlay_and_inserts_newline() {
    let (mut app, _command_rx) = make_app();
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay(), "command overlay active");

    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(!app.composer().has_overlay(), "overlay closed after Shift+Enter");
    assert_eq!(app.composer().text(), "/\n");
}

#[test]
fn ctrl_j_closes_file_overlay_and_inserts_newline() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    app.on_key(key(KeyCode::Char('@')));
    assert!(app.composer().has_overlay(), "file overlay active");

    app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    assert!(!app.composer().has_overlay(), "overlay closed after Ctrl-J");
    assert_eq!(app.composer().text(), "@\n");
}

#[test]
fn alt_enter_closes_command_overlay_and_inserts_newline() {
    let (mut app, _command_rx) = make_app();
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    assert!(!app.composer().has_overlay(), "overlay closed after Alt+Enter");
    assert_eq!(app.composer().text(), "/\n");
}

#[test]
fn clear_no_duplicate_generation_bump() {
    let (mut app, mut command_rx) = make_app();

    let gen_before_clear = app.transcript_generation();

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_eq!(
        app.transcript_generation(),
        gen_before_clear.wrapping_add(1),
        "transcript_generation should only increment once (in NewSessionCreated), not both in dispatch and event"
    );
}

#[test]
fn new_session_send_failure_shows_transcript_error() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app();

    fail_signal.store(true, Ordering::Relaxed);
    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("new session") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for new_session failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after new_session failure");
}

#[test]
fn list_sessions_send_failure_shows_transcript_error() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app();

    fail_signal.store(true, Ordering::Relaxed);
    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("list sessions") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for list_sessions failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after list_sessions failure");
}

#[test]
fn load_session_send_failure_cleans_up_buffer_and_shows_error() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("load session") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for load_session failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after load_session failure");
}

#[test]
fn session_preview_loaded_for_selected_session() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));

    let preview_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&preview_cmd, PromptCommand::SessionPreview(params) if params.session_id == "sess-1"),
        "expected preview for first session, got {preview_cmd:?}"
    );
}

#[test]
fn session_preview_updated_when_selection_changes() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = command_rx.try_recv().unwrap();

    app.on_key(key(KeyCode::Down));

    let preview_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&preview_cmd, PromptCommand::SessionPreview(params) if params.session_id == "sess-2"),
        "expected preview for second session after moving down, got {preview_cmd:?}"
    );
}

#[test]
fn stale_preview_does_not_replace_current() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = command_rx.try_recv().unwrap();

    app.on_key(key(KeyCode::Down));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::SessionPreviewLoaded(session_preview_response("sess-1")));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("hello"), "stale preview should not be shown:\n{viewport}");
}

#[test]
fn session_preview_failure_shows_error() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::SessionPreviewFailed {
        session_id: "sess-1".to_string(),
        error: "server unreachable".to_string(),
    });

    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("server unreachable"), "expected error in preview:\n{viewport}");
}

#[test]
fn session_loading_buffer_queues_updates_then_replays() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_update_for("loaded", user_message_chunk("buffered message")));
    app.on_acp_event(session_update_for(
        "loaded",
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
            "buffered agent",
        )))),
    ));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("buffered message"), "buffered updates should not render yet:\n{viewport}");
    assert!(!viewport.contains("buffered agent"), "buffered updates should not render yet:\n{viewport}");

    app.on_acp_event(session_loaded("loaded", vec![select_option("model", "sonnet")]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("buffered message"), "buffered updates should be replayed:\n{viewport}");
    assert!(viewport.contains("buffered agent"), "buffered updates should be replayed:\n{viewport}");
}

#[test]
fn loaded_session_uses_server_config_values() {
    let options = vec![select_option("model", "opus"), mode_option("plan", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) =
        make_app_with_metadata(std::path::PathBuf::from("."), acp::SessionCapabilities::new(), options, Vec::new());

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_loaded(
        "loaded",
        vec![select_option("model", "sonnet"), mode_option("code", &["code", "plan", "ask"])],
    ));

    let config = app.config_options();
    let acp::SessionConfigKind::Select(model) = &config[0].kind else {
        panic!("expected model");
    };
    assert_eq!(model.current_value.0.as_ref(), "sonnet");
    let acp::SessionConfigKind::Select(mode) = &config[1].kind else {
        panic!("expected mode");
    };
    assert_eq!(mode.current_value.0.as_ref(), "code");
}

#[test]
fn connection_closed_cancels_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_session_picker());
    assert!(app.exit_requested());
}

#[test]
fn session_list_error_shows_in_transcript() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "list sessions failed".to_string() });

    let items = app.drain_finalized();
    let has_error =
        items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("list sessions failed")));
    assert!(has_error, "expected visible transcript error, got {items:?}");
}

#[test]
fn builtin_clear_appears_in_command_picker() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/clear"), "built-in /clear should be in command picker:\n{viewport}");
    assert!(viewport.contains("/resume"), "built-in /resume should be in command picker:\n{viewport}");
}

#[test]
fn narrow_terminal_renders_session_picker_without_preview_pane() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));

    let mut terminal = make_terminal_with_width(60);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Session One"), "narrow picker should show session list:\n{viewport}");
    assert!(!viewport.contains("Session preview"), "narrow picker should hide preview pane:\n{viewport}");
}

#[test]
fn new_modal_replaces_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (cx, mut peer) = test_connection().await;
        let (responder, _response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
    });

    assert!(!app.has_session_picker());
}
