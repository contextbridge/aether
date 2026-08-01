use super::support::*;

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

    assert_eq!(composer.completion().map(|overlay| overlay.query()), Some("sea"));
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

    assert_eq!(composer.completion().map(|overlay| overlay.query()), Some("main"));
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
    let mut ui = TestUiBuilder::new().working_dir(directory.path().to_path_buf()).build();

    ui.key(key(KeyCode::Char('@')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Char('c')));
    ui.draw();

    assert!(ui.viewport_text().contains("context.txt"));
    assert!(!ui.history_text().contains("context.txt"));
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
    let mut ui = TestUi::new();
    let heading = ui.theme().heading;
    ui.submit("render markdown");
    ui.acp_event(text_chunk("# Heading\n\n**boldword** and *italicword*"));

    ui.draw();

    let conversation = ui.conversation();
    assert!(buffer_text(&conversation).contains("# Heading"));
    assert!(has_cell(&conversation, "H", |cell| cell.fg == heading && cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "b", |cell| cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "i", |cell| cell.modifier.contains(Modifier::ITALIC)));

    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();
    ui.draw();

    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("Heading").count(), 1);
    assert!(!conversation.contains("**boldword**"));
}

#[test]
fn fenced_code_info_string_is_syntax_highlighted_with_token_colors() {
    let mut ui = TestUi::new();
    let code_background = ui.theme().code_bg;
    ui.submit("show code");
    ui.acp_event(text_chunk("```rust title=\"example.rs\"\nfn highlighted() {}\n```"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    ui.draw();

    let conversation = ui.conversation();
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
    let mut ui = TestUi::new();
    let code_background = ui.theme().code_bg;
    ui.submit("show code");
    ui.acp_event(text_chunk("Example:\n"));
    ui.draw();
    ui.acp_event(text_chunk("```rust\n"));
    ui.draw();
    ui.acp_event(text_chunk("fn highlighted() {}\n"));
    ui.draw();

    assert!(has_cell(&ui.viewport(), "f", |cell| cell.bg == code_background));

    ui.acp_event(text_chunk("```\n"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();
    ui.draw();

    let conversation = ui.conversation();
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
    let mut ui = TestUi::new();
    let code_background = ui.theme().code_bg;
    ui.submit("show documented code");
    ui.acp_event(text_chunk("```python\n# Documentation\nif condition:\n    pass\n```"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    ui.draw();

    let conversation = ui.conversation();
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
    let mut ui = TestUi::new();
    let removed_background = ui.theme().diff_removed_bg;
    let added_background = ui.theme().diff_added_bg;
    ui.submit("edit file");
    ui.acp_event(tool_call("edit-1", "Edit src/main.rs"));
    ui.acp_event(tool_completed_with_diff("edit-1"));

    ui.draw();
    ui.draw();

    let conversation = ui.conversation();
    let text = buffer_text(&conversation);
    assert_eq!(text.matches("old_name").count(), 1);
    assert_eq!(text.matches("new_name").count(), 1);
    assert!(has_cell(&conversation, "-", |cell| cell.bg == removed_background));
    assert!(has_cell(&conversation, "+", |cell| cell.bg == added_background));
}

#[test]
fn wide_diff_uses_side_by_side_layout() {
    let mut ui = TestUi::with_dimensions(160, 15);
    ui.submit("edit file");
    ui.acp_event(tool_call("edit-1", "Edit src/main.rs"));
    ui.acp_event(tool_completed_with_diff("edit-1"));

    ui.draw();

    let text = buffer_text(&ui.conversation());
    assert!(text.lines().any(|line| line.contains("old_name") && line.contains("new_name")), "{text}");
}

#[test]
fn wide_diff_marks_truncated_panel_content() {
    let mut ui = TestUi::with_dimensions(160, 15);
    ui.submit("edit file");
    ui.acp_event(tool_call("edit-1", "Edit src/main.rs"));
    ui.acp_event(tool_completed_with_diff_contents(
        "edit-1",
        &format!("old_{}\n", "x".repeat(80)),
        &format!("new_{}\n", "y".repeat(80)),
    ));

    ui.draw();

    let text = buffer_text(&ui.conversation());
    assert!(text.contains('…'), "expected visibly truncated split diff:\n{text}");
}

#[test]
fn markdown_blockquote_prefixes_inline_code_first_content() {
    let mut ui = TestUi::with_dimensions(44, 15);
    let code_background = ui.theme().code_bg;
    ui.submit("quote this");
    ui.acp_event(text_chunk("> `quoted`"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "quoted").expect("blockquote content should render");
    assert_eq!(row_text(&conversation, row).trim_end(), "    quoted");
    assert!(
        has_cell(&conversation, "q", |cell| cell.bg == code_background),
        "inline code keeps its background inside a blockquote"
    );
}

#[test]
fn markdown_horizontal_rule_uses_available_width() {
    let mut ui = TestUi::with_dimensions(24, 15);
    ui.submit("add a rule");
    ui.acp_event(text_chunk("---"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "─").expect("horizontal rule should render");
    assert_eq!(row_text(&conversation, row).trim_end(), format!("  {}", "─".repeat(20)));
}

#[test]
fn transcript_markdown_wraps_at_word_boundaries() {
    let mut ui = TestUi::with_dimensions(13, 15);
    ui.submit("wrap words");
    ui.acp_event(text_chunk("alpha beta gamma"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let alpha = row_containing(&conversation, "alpha").expect("first word should render");
    let beta = row_containing(&conversation, "beta").expect("second word should render");
    let gamma = row_containing(&conversation, "gamma").expect("third word should render");
    assert_eq!(beta, alpha + 1);
    assert_eq!(gamma, beta + 1);
    assert_eq!(row_text(&conversation, alpha).trim_end(), "  alpha");
}

#[test]
fn transcript_markdown_separates_a_heading_from_its_paragraph() {
    let mut ui = TestUi::with_dimensions(44, 15);
    ui.submit("heading test");
    ui.acp_event(text_chunk("# Heading\n\nParagraph text."));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let heading = row_containing(&conversation, "Heading").expect("heading should render");
    let paragraph = row_containing(&conversation, "Paragraph").expect("paragraph should render");
    assert_eq!(paragraph, heading + 2, "heading and paragraph need one blank row");
    assert_eq!(row_text(&conversation, heading).trim_end(), "  # Heading");
    assert_eq!(row_text(&conversation, paragraph).trim_end(), "  Paragraph text.");
}

#[test]
fn transcript_wrapping_expands_tabs() {
    let mut ui = TestUi::with_dimensions(8, 15);
    ui.submit("hello");
    ui.acp_event(text_chunk("a\tb"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let a_row = row_containing(&conversation, "a").expect("tab-expanded row should render");
    let b_row = row_containing(&conversation, "b").expect("wrapped word should render");
    assert_eq!(b_row, a_row + 1, "the tab-expanded word must wrap onto its own row");
    assert!(row_text(&conversation, a_row).starts_with("  a   "), "tab must expand to the next tab stop");
}

#[test]
fn transcript_wrapping_never_exceeds_a_one_column_allocation() {
    let mut ui = TestUi::with_dimensions(5, 15);
    ui.submit("x");
    ui.acp_event(text_chunk("界"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "…").expect("wide glyph should truncate to an ellipsis");
    assert_eq!(row_text(&conversation, row).trim_end(), "  …");
    assert!(!has_cell(&conversation, "界", |_| true), "the wide glyph must not spill past the one-column allocation");
}

#[test]
fn trailing_newline_does_not_add_an_empty_user_content_row() {
    let mut ui = TestUi::new();
    let user_background = ui.theme().sidebar_bg;
    ui.type_text("hello");
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    ui.key(key(KeyCode::Enter));

    ui.draw();

    let styled_rows = rows_with_background(&ui.conversation(), user_background);
    assert_eq!(styled_rows, 3);
}

#[test]
fn markdown_renders_lists_strikethrough_and_tables() {
    let mut ui = TestUi::with_dimensions(44, 15);
    let markdown = "- first\n- second\n\n~~removed~~\n\n| Name | Value |\n| --- | --- |\n| alpha | beta |";
    ui.submit("render table");
    ui.acp_event(text_chunk(markdown));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let conversation = ui.conversation();
    let text = buffer_text(&conversation);
    assert!(text.contains("• first"), "{text}");
    assert!(text.contains("• second"), "{text}");
    assert!(text.contains("| Name  | Value |"), "{text}");
    assert!(text.contains("|-------|-------|"), "{text}");
    assert!(text.contains("| alpha | beta  |"), "{text}");
    assert!(has_cell(&conversation, "r", |cell| cell.modifier.contains(Modifier::CROSSED_OUT)));
}

/// The streaming tail's markdown is cached per frame: a message that grows one
/// chunk at a time reuses the render of its unchanged prefix, and once the
/// prompt finalizes only the final text is committed — no intermediate prefix
/// ever leaks into the conversation.
#[test]
fn streaming_markdown_reuses_unchanged_renders_and_finalizes_once() {
    let mut ui = TestUi::new();
    ui.submit("stream cache");

    ui.acp_event(text_chunk("# First"));
    ui.draw();
    ui.assert_viewport_contains("# First");

    ui.acp_event(text_chunk("\n\nSecond paragraph"));
    ui.draw();
    ui.assert_viewport_contains("# First");
    ui.assert_viewport_contains("Second paragraph");

    // A frame with nothing new draws the same output: the unchanged prefix's
    // render is reused instead of being rebuilt.
    let before = ui.viewport_text();
    ui.draw();
    assert_eq!(ui.viewport_text(), before);

    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();
    ui.draw();

    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("# First").count(), 1, "intermediate prefix leaked:\n{conversation}");
    assert_eq!(conversation.matches("Second paragraph").count(), 1);
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
    let mut ui = TestUi::new();
    let mut markdown = String::new();
    for index in 0..40 {
        writeln!(markdown, "paragraph-{index}\n").unwrap();
    }
    ui.submit("long response");
    ui.acp_event(text_chunk(&markdown));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    ui.draw();

    let conversation = ui.conversation();
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
fn one_row_inline_viewport_draws_without_panicking() {
    let mut ui = TestUi::with_dimensions(40, 3);

    ui.draw();

    assert_eq!(ui.terminal_mut().get_frame().area().height, 1);
}

/// Scrollback leaves for the terminal's own history before the viewport is
/// repainted, so the frame is drawn exactly once, already in its final
/// position. Drawing first would briefly show a viewport that no longer holds
/// the drained lines above a history that does not hold them yet.
#[test]
fn history_is_inserted_before_the_live_viewport_is_drawn() {
    let mut ui = TestUi::with_backend(RecordingBackend::new(40, 15));
    ui.draw();
    ui.terminal_mut().backend_mut().events.clear();

    let mut response = String::new();
    for index in 0..20 {
        writeln!(response, "line-{index}\n").unwrap();
    }
    ui.submit("hello");
    ui.acp_event(text_chunk(&response));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    let events: Vec<BackendEvent> = ui.terminal_mut().backend().events.clone();
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
    let mut ui = TestUi::new();
    ui.draw();

    ui.submit("hello");
    ui.draw();

    let scrollback = ui.history_text();
    assert!(!scrollback.contains("working"), "{scrollback}");
    assert!(!scrollback.contains("esc to cancel"), "{scrollback}");
    assert!(!scrollback.contains("aether"), "{scrollback}");
}

#[test]
fn history_waits_until_resized_viewport_has_scrollback_room() {
    let mut ui = TestUi::new();
    ui.draw();

    ui.resize(40, 10);
    ui.submit("queued while small");
    ui.draw();

    assert!(!ui.history_text().contains("queued while small"));
    assert!(ui.viewport_text().contains("queued while small"));

    ui.resize(40, 15);
    ui.draw();

    assert!(ui.app().pending_items().is_empty());
    assert!(ui.viewport_text().contains("queued while small"));
}
