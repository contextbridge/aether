use super::support::*;
use wisp_next::test_support::picker::CompletionOverlay;

#[test]
fn app_exposes_initial_config_option_selections() {
    let options =
        vec![select_option("model", "opus"), select_option("mode", "plan"), select_option("reasoning", "high")];
    let (app, _command_rx) = AppBuilder::new().config_options(options).build();

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
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "opus")]).build();

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
    let (mut app, _command_rx) = AppBuilder::new().auth_methods(initial).build();
    let updated = vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("updated", "Updated"))];

    app.on_acp_event(AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: updated }));

    assert_eq!(app.auth_methods().len(), 1);
    assert_eq!(app.auth_methods()[0].id().0.as_ref(), "updated");
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

    assert_eq!(composer.completion_ref().map(CompletionOverlay::query), Some("sea"));
    assert!(completion_contains(&mut composer, "/search"));

    let selected = composer.accept_command().unwrap();
    assert_eq!(selected.name, "search");
    assert_eq!(composer.text(), "/search");
    assert!(!composer.has_completion());
}

#[test]
fn file_picker_filters_and_inserts_a_mention() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("src")).unwrap();
    std::fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(directory.path().join("README.md"), "# demo\n").unwrap();

    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, directory.path());
    composer.insert_str("main");
    composer.refresh_overlay_query();

    assert_eq!(composer.completion_ref().map(CompletionOverlay::query), Some("main"));
    assert!(completion_contains(&mut composer, "src/main.rs"));

    let selected = composer.accept_file().unwrap();
    assert_eq!(selected.display_name, "src/main.rs");
    assert_eq!(composer.text(), "@src/main.rs ");
    assert!(!composer.has_completion());
}

#[test]
fn selected_file_is_sent_as_an_acp_resource_attachment() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("context.txt"), "attached context").unwrap();
    let (mut app, mut command_rx) = make_app_in(directory.path().to_path_buf());

    app.on_key(key(KeyCode::Char('@')));
    settle_tasks(&mut app);
    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.composer().text(), "@context.txt ");
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

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
    let mut renderer = Presenter::new(&UiSettings::default());

    app.on_key(key(KeyCode::Char('@')));
    settle_tasks(&mut app);
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
fn composer_edits_unicode_without_splitting_graphemes() {
    let mut composer = Composer::new();
    composer.insert_str("a界🙂");
    composer.move_left();
    composer.backspace();

    assert_eq!(composer.text(), "a🙂");
    assert_eq!(composer.cursor_position(), (0, 1));

    composer.insert_newline();
    composer.insert_str("é");
    assert_eq!(composer.cursor_position(), (1, 1));
    assert_eq!(composer.take_submission().0, "a\né🙂");
}

#[test]
fn markdown_styles_stream_live_and_finalize_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
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
fn fenced_code_info_string_is_syntax_highlighted_with_token_colors() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    let code_background = renderer.theme().code_bg;
    submit_prompt(&mut app, "show code");
    app.on_acp_event(text_chunk("```rust title=\"example.rs\"\nfn highlighted() {}\n```"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    assert!(buffer_text(&conversation).contains("fn highlighted()"));
    assert!(has_cell(&conversation, "f", |cell| cell.bg == code_background));
    let keyword_color = conversation
        .content
        .iter()
        .find(|cell| cell.symbol() == "f" && cell.bg == code_background)
        .map(|cell| cell.fg)
        .expect("rendered Rust keyword");
    let identifier_color = conversation
        .content
        .iter()
        .find(|cell| cell.symbol() == "h" && cell.bg == code_background)
        .map(|cell| cell.fg)
        .expect("rendered Rust identifier");
    assert_ne!(keyword_color, identifier_color, "Rust keywords and identifiers should use distinct colors");
}

#[test]
fn fenced_code_streamed_across_chunks_is_syntax_highlighted() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    let code_background = renderer.theme().code_bg;
    submit_prompt(&mut app, "show code");
    app.on_acp_event(text_chunk("Example:\n"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    app.on_acp_event(text_chunk("```rust\n"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    app.on_acp_event(text_chunk("fn highlighted() {}\n"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(has_cell(&viewport_buffer(&mut terminal), "f", |cell| cell.bg == code_background));

    app.on_acp_event(text_chunk("```\n"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    let row = row_containing(&conversation, "fn highlighted()").expect("rendered code line");
    let cells = (conversation.area.left()..conversation.area.right())
        .filter_map(|x| conversation.cell((x, row)))
        .collect::<Vec<_>>();
    let keyword = cells.iter().find(|cell| cell.symbol() == "f").expect("fn keyword");
    let identifier = cells.iter().find(|cell| cell.symbol() == "h").expect("highlighted identifier");
    assert_eq!(keyword.bg, code_background);
    assert_eq!(identifier.bg, code_background);
    assert_ne!(keyword.fg, identifier.fg, "streamed fences must keep syntax highlighting");
}

#[test]
fn fenced_code_line_comment_does_not_consume_following_code() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    let code_background = renderer.theme().code_bg;
    submit_prompt(&mut app, "show documented code");
    app.on_acp_event(text_chunk("```python\n# Documentation\nif condition:\n    pass\n```"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    let row = row_containing(&conversation, "if condition").expect("Python conditional after line comment");
    let cells = (conversation.area.left()..conversation.area.right())
        .filter_map(|x| conversation.cell((x, row)))
        .collect::<Vec<_>>();
    let keyword_color = cells.iter().find(|cell| cell.symbol() == "i").expect("if keyword").fg;
    let identifier_color = cells.iter().find(|cell| cell.symbol() == "c").expect("condition identifier").fg;
    assert_ne!(keyword_color, identifier_color, "line comments must not disable highlighting on following lines");
    assert!(cells.iter().all(|cell| cell.bg == code_background || cell.symbol() == " "));
}

#[test]
fn completed_tool_diff_is_themed_and_rendered_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
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
    let mut renderer = Presenter::new(&UiSettings::default());
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
    let mut renderer = Presenter::new(&UiSettings::default());
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
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text("> `quoted`".to_string())], None, 40, 0, 0);

    assert_eq!(line_text(&lines[0]), "  quoted");
}

#[test]
fn markdown_horizontal_rule_uses_available_width() {
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text("---".to_string())], None, 20, 0, 0);

    assert_eq!(line_text(&lines[0]), "─".repeat(20));
}

#[test]
fn transcript_markdown_wraps_at_word_boundaries() {
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text("alpha beta gamma".to_string())], None, 9, 0, 0);

    assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>(), ["alpha", "beta", "gamma"]);
}

#[test]
fn transcript_markdown_separates_a_heading_from_its_paragraph() {
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(
        Segment::Committed,
        &[HistoryItem::Text("# Heading\n\nParagraph text.".to_string())],
        None,
        40,
        0,
        0,
    );

    assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>(), ["# Heading", "", "Paragraph text."]);
}

#[test]
fn transcript_wrapping_expands_tabs() {
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text("a\tb".to_string())], None, 4, 0, 0);

    assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>(), ["a   ", "b"]);
}

#[test]
fn transcript_wrapping_never_exceeds_a_one_column_allocation() {
    let mut renderer = Presenter::new(&UiSettings::default());

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text("界".to_string())], None, 1, 0, 0);

    assert!(lines.iter().all(|line| line.width() <= 1), "{lines:?}");
    assert_eq!(line_text(&lines[0]), "…");
}

#[test]
fn trailing_newline_does_not_add_an_empty_user_content_row() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
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
    let mut renderer = Presenter::new(&UiSettings::default());
    let markdown = "- first\n- second\n\n~~removed~~\n\n| Name | Value |\n| --- | --- |\n| alpha | beta |";

    let lines = renderer.lines(Segment::Committed, &[HistoryItem::Text(markdown.to_string())], None, 40, 0, 0);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(text.contains("• first"), "{text}");
    assert!(text.contains("• second"), "{text}");
    assert!(text.contains("| Name  | Value |"), "{text}");
    assert!(text.contains("|-------|-------|"), "{text}");
    assert!(text.contains("| alpha | beta  |"), "{text}");
    assert!(
        lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content.contains("removed") && span.style.add_modifier.contains(Modifier::CROSSED_OUT)
        })
    );
}

#[test]
fn default_theme_is_sage() {
    let theme = Theme::default();

    assert_eq!(theme.syntect().name.as_deref(), Some("Sage"));
    assert_eq!(theme.background, Color::Rgb(0x15, 0x1d, 0x1f));
    assert_eq!(theme.text_primary, Color::Rgb(0xd4, 0xdd, 0xd6));
    assert_eq!(theme.accent, Color::Rgb(0x8f, 0xbc, 0xb0));
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
    let mut renderer = Presenter::new(&UiSettings::default());
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
        r#"{"contentPadding":4,"theme":{"file":"nord.tmTheme","future":true},"statusLine":{"left":[{"type":"cwd"}],"right":[{"type":"agent"}]}}"#,
    )
    .unwrap();

    assert_eq!(settings.content_padding, Some(4));
    assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
    assert!(settings.status_line.is_some());
    let sl = settings.status_line.unwrap();
    assert_eq!(sl.left, Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]));
    assert_eq!(sl.right, Some(vec![StatusLineSegmentConfig::Agent]));
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
    let mut renderer = Presenter::new(&UiSettings::default());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert_eq!(terminal.get_frame().area().height, 1);
}

/// Scrollback leaves for the terminal's own history before the viewport is
/// repainted, so the frame is drawn exactly once, already in its final
/// position. Drawing first would briefly show a viewport that no longer holds
/// the drained lines above a history that does not hold them yet.
#[test]
fn history_is_inserted_before_the_live_viewport_is_drawn() {
    let (mut app, _command_rx) = make_app();
    let backend = RecordingBackend::new(40, 15);
    let mut terminal =
        Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) })
            .unwrap();
    let mut renderer = Presenter::new(&UiSettings::default());
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
    assert!(insert < draw, "expected history insertion before viewport draw: {events:?}");
    assert_eq!(
        events.iter().filter(|event| **event == BackendEvent::ShowCursor).count(),
        1,
        "the frame should be drawn once, not once per side of the insertion: {events:?}"
    );
}

#[test]
fn status_line_is_never_inserted_into_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = Terminal::with_options(
        TestBackend::new(40, 15),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) },
    )
    .unwrap();
    let mut renderer = Presenter::new(&UiSettings::default());
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
    let mut renderer = Presenter::new(&UiSettings::default());
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
