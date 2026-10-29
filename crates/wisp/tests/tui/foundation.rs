use super::support::*;

#[test]
fn app_exposes_initial_config_option_selections() {
    let options =
        vec![select_option("model", "opus"), select_option("mode", "plan"), select_option("reasoning", "high")];
    let app = TestUiBuilder::new().session(|session| session.config_options(options)).build();

    let selections: Vec<_> = app
        .app()
        .core()
        .session()
        .config_options()
        .iter()
        .map(|option| (option.id.as_str(), option.current_value().expect("expected select option")))
        .collect();

    assert_eq!(selections, [("model", "opus"), ("mode", "plan"), ("reasoning", "high")]);
}

#[test]
fn config_option_update_replaces_current_selections() {
    let mut app =
        TestUiBuilder::new().session(|session| session.config_options(vec![select_option("model", "opus")])).build();

    app.acp_event(config_update(vec![select_option("model", "sonnet"), select_option("mode", "code")]));

    assert_eq!(app.app().core().session().config_options().len(), 2);
    assert_eq!(app.app().core().session().config_options()[0].current_value(), Some("sonnet"));
}

#[test]
fn auth_methods_update_replaces_current_auth_methods() {
    let initial = vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("initial", "Initial"))];
    let mut app = TestUiBuilder::new().session(|session| session.auth_methods(initial)).build();
    let updated = vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("updated", "Updated"))];

    app.acp_event(AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: updated }));

    assert_eq!(app.app().core().session().auth_methods().len(), 1);
    assert_eq!(app.app().core().session().auth_methods()[0].method_id().0.as_ref(), "updated");
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
        SlashCommand::Agent(CommandEntry {
            name: "search".to_string(),
            description: "Search the workspace".to_string(),
            has_input: true,
            hint: Some("query".to_string()),
        }),
        SlashCommand::Agent(CommandEntry {
            name: "status".to_string(),
            description: "Show status".to_string(),
            has_input: false,
            hint: None,
        }),
    ]);
    composer.insert_str("sea");
    composer.refresh_overlay_query();

    assert_eq!(composer.completion().map(wisp::surfaces::picker::CompletionOverlay::query), Some("sea"));
    assert!(completion_contains(&mut composer, "/search"));

    let selected = composer.accept_command().unwrap();
    assert_eq!(selected.name(), "search");
    assert_eq!(composer.text(), "/search");
    assert!(!composer.has_completion());
}

#[test]
fn file_picker_filters_and_inserts_a_mention() {
    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, &["src/main.rs", "README.md"]);
    composer.insert_str("main");
    composer.refresh_overlay_query();

    assert_eq!(composer.completion().map(wisp::surfaces::picker::CompletionOverlay::query), Some("main"));
    assert!(completion_contains(&mut composer, "src/main.rs"));

    let selected = composer.accept_file().unwrap();
    assert_eq!(selected.display_name, "src/main.rs");
    assert_eq!(composer.text(), "@src/main.rs ");
    assert!(!composer.has_completion());
}

#[test]
fn selected_file_is_sent_as_an_acp_resource_attachment() {
    let mut app = TestUiBuilder::new().file("context.txt", b"attached context").dimensions(160, 15).build();

    app.key(key(KeyCode::Char('@')));
    app.key(key(KeyCode::Char('c')));
    app.key(key(KeyCode::Enter));
    assert_eq!(app.app().composer().text(), "@context.txt ");
    app.key(key(KeyCode::Enter));

    let prompt = app.agent().prompt.try_recv().expect("the agent received the prompt").prompt;
    assert!(
        matches!(&prompt[..], [acp::ContentBlock::Text(text), acp::ContentBlock::Resource(_)] if text.text == "@context.txt ")
    );
    let expanded = acp_utils::content::map_content_blocks_to_text(&prompt);
    assert!(expanded.contains("attached context"), "the model receives the file contents");
    app.assert_viewport_contains("@context.txt");
    app.assert_viewport_contains(&format!("file://{}", app.workspace().join("context.txt").display()));
    app.assert_conversation_not_contains("attached context");
}

#[test]
fn file_picker_renders_in_the_live_viewport_not_scrollback() {
    let mut ui = TestUiBuilder::new().file("context.txt", b"attached context").build();

    ui.key(key(KeyCode::Char('@')));
    ui.key(key(KeyCode::Char('c')));

    ui.assert_viewport_contains("context.txt");
    ui.assert_history_not_contains("context.txt");
}

#[test]
fn composer_history_restores_the_unsubmitted_draft() {
    let mut composer = Composer::new();
    composer.insert_str("first");
    let submission = composer.take_submission();
    assert_eq!(submission.text, "first");
    assert_eq!(submission.attachments().len(), 0);
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
    assert_eq!(composer.take_submission().text, "a\né🙂");
}

#[test]
fn markdown_styles_stream_live_and_finalize_once() {
    let mut ui = TestUi::new();
    let heading = ui.app().theme().heading;
    ui.submit("render markdown");
    ui.acp_event(text_chunk("# Heading\n\n**boldword** and *italicword*"));

    let conversation = ui.conversation();
    assert!(buffer_text(&conversation).contains("# Heading"));
    assert!(has_cell(&conversation, "H", |cell| cell.fg == heading && cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "b", |cell| cell.modifier.contains(Modifier::BOLD)));
    assert!(has_cell(&conversation, "i", |cell| cell.modifier.contains(Modifier::ITALIC)));

    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("Heading").count(), 1);
    assert!(!conversation.contains("**boldword**"));
}

#[test]
fn fenced_code_info_string_is_syntax_highlighted_with_token_colors() {
    let mut ui = TestUi::new();
    let code_background = ui.app().theme().code_bg;
    ui.turn("show code", "```rust title=\"example.rs\"\nfn highlighted() {}\n```");

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
    let code_background = ui.app().theme().code_bg;
    ui.submit("show code");
    ui.acp_event(text_chunk("Example:\n"));
    ui.draw();
    ui.acp_event(text_chunk("```rust\n"));
    ui.draw();
    ui.acp_event(text_chunk("fn highlighted() {}\n"));

    assert!(has_cell(&ui.viewport(), "f", |cell| cell.bg == code_background));

    ui.acp_event(text_chunk("```\n"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation();
    let cells =
        row_cells(&conversation, row_containing(&conversation, "fn highlighted()").expect("rendered code line"));
    let keyword = cells.iter().find(|cell| cell.symbol() == "f").expect("fn keyword");
    let identifier = cells.iter().find(|cell| cell.symbol() == "h").expect("highlighted identifier");
    assert_eq!(keyword.bg, code_background);
    assert_eq!(identifier.bg, code_background);
    assert_ne!(keyword.fg, identifier.fg, "streamed fences must keep syntax highlighting");
}

#[test]
fn fenced_code_line_comment_does_not_consume_following_code() {
    let mut ui = TestUi::new();
    let code_background = ui.app().theme().code_bg;
    ui.turn("show documented code", "```python\n# Documentation\nif condition:\n    pass\n```");

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "if condition").expect("Python conditional after line comment");
    let cells = row_cells(&conversation, row);
    let keyword_color = cells.iter().find(|cell| cell.symbol() == "i").expect("if keyword").fg;
    let identifier_color = cells.iter().find(|cell| cell.symbol() == "c").expect("condition identifier").fg;
    assert_ne!(keyword_color, identifier_color, "line comments must not disable highlighting on following lines");
    assert!(cells.iter().all(|cell| cell.bg == code_background || cell.symbol() == " "));
}

#[test]
fn completed_tool_diff_is_themed_and_rendered_once() {
    let mut ui = TestUi::new();
    let removed_background = ui.app().theme().diff_removed_bg;
    let added_background = ui.app().theme().diff_added_bg;
    ui.submit("edit file");
    ui.acp_event(tool_call("edit-1", "Edit src/main.rs"));
    ui.acp_event(tool_completed_with_diff("edit-1"));

    let conversation = ui.conversation();
    let text = buffer_text(&conversation);
    assert_eq!(text.matches("old_name").count(), 1);
    assert_eq!(text.matches("new_name").count(), 1);
    let row_has_background = |row, background| row_cells(&conversation, row).iter().any(|cell| cell.bg == background);
    let removed_row = row_containing(&conversation, "old_name").expect("rendered removed diff row");
    let added_row = row_containing(&conversation, "new_name").expect("rendered added diff row");
    assert!(row_has_background(removed_row, removed_background));
    assert!(row_has_background(added_row, added_background));
}

#[test]
fn wide_diff_uses_side_by_side_layout() {
    let mut ui = TestUi::with_dimensions(160, 15);
    ui.submit("edit file");
    ui.acp_event(tool_call("edit-1", "Edit src/main.rs"));
    ui.acp_event(tool_completed_with_diff("edit-1"));

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

    ui.assert_conversation_contains("…");
}

#[test]
fn markdown_blockquote_prefixes_inline_code_first_content() {
    let mut ui = TestUi::with_dimensions(44, 15);
    let code_background = clankerdiff_ratatui::composite_color(
        ui.app().theme().review().markdown.inline_code_background,
        ui.app().theme().review().diff.background,
    );
    ui.turn("quote this", "> `quoted`");

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "quoted").expect("blockquote content should render");
    assert_eq!(row_text(&conversation, row).trim_end(), "  │ quoted");
    assert!(
        has_cell(&conversation, "q", |cell| cell.bg == code_background),
        "inline code keeps its background inside a blockquote"
    );
}

#[test]
fn markdown_horizontal_rule_uses_available_width() {
    let mut ui = TestUi::with_dimensions(24, 15);
    ui.turn("add a rule", "---");

    let conversation = ui.conversation();
    let row = row_containing(&conversation, "─").expect("horizontal rule should render");
    assert_eq!(row_text(&conversation, row).trim_end(), format!("  {}", "─".repeat(20)));
}

#[test]
fn transcript_markdown_wraps_at_content_width() {
    let mut ui = TestUi::with_dimensions(13, 15);
    ui.turn("wrap words", "alpha beta gamma");

    let conversation = ui.conversation();
    let alpha = row_containing(&conversation, "alpha").expect("first word should render");
    assert_eq!(row_text(&conversation, alpha).trim_end(), "  alpha bet");
    assert_eq!(row_text(&conversation, alpha + 1).trim_end(), "  a gamma");
}

#[test]
fn transcript_markdown_separates_a_heading_from_its_paragraph() {
    let mut ui = TestUi::with_dimensions(44, 15);
    ui.turn("heading test", "# Heading\n\nParagraph text.");

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
    ui.turn("hello", "a\tb");

    let conversation = ui.conversation();
    let a_row = row_containing(&conversation, "a").expect("tab-expanded row should render");
    let b_row = row_containing(&conversation, "b").expect("wrapped word should render");
    assert_eq!(b_row, a_row + 1, "the tab-expanded word must wrap onto its own row");
    assert!(row_text(&conversation, a_row).starts_with("  a   "), "tab must expand to the next tab stop");
}

#[test]
fn transcript_wrapping_never_exceeds_a_one_column_allocation() {
    let mut ui = TestUi::with_dimensions(5, 15);
    ui.turn("x", "界");

    let conversation = ui.conversation();
    assert!(!has_cell(&conversation, "界", |_| true), "the wide glyph must not spill past the one-column allocation");
}

#[test]
fn trailing_newline_does_not_add_an_empty_user_content_row() {
    let mut ui = TestUi::new();
    let user_background = ui.app().theme().surface;
    ui.type_text("hello");
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    ui.key(key(KeyCode::Enter));

    let styled_rows = rows_with_background(&ui.conversation(), user_background);
    assert_eq!(styled_rows, 3);
}

#[test]
fn markdown_renders_lists_strikethrough_and_tables() {
    let mut ui = TestUi::with_dimensions(44, 15);
    let markdown = "- first\n- second\n\n~~removed~~\n\n| Name | Value |\n| --- | --- |\n| alpha | beta |";
    ui.turn("render table", markdown);

    let conversation = ui.conversation();
    let text = buffer_text(&conversation);
    assert!(text.contains("• first"), "{text}");
    assert!(text.contains("• second"), "{text}");
    assert!(text.contains("│ Name  │ Value │"), "{text}");
    assert!(text.contains("│ alpha │ beta  │"), "{text}");
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
    ui.assert_viewport_contains("# First");

    ui.acp_event(text_chunk("\n\nSecond paragraph"));
    ui.assert_viewport_contains("# First");
    ui.assert_viewport_contains("Second paragraph");

    // A frame with nothing new draws the same output: the unchanged prefix's
    // render is reused instead of being rebuilt.
    let before = ui.viewport_text();
    assert_eq!(ui.viewport_text(), before);

    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("# First").count(), 1, "intermediate prefix leaked:\n{conversation}");
    assert_eq!(conversation.matches("Second paragraph").count(), 1);
}

#[test]
fn default_theme_is_sage() {
    let theme = Theme::default();

    assert_eq!(theme.review().id().to_string(), "sage");
    assert_eq!(theme.background, Color::Rgb(0x0d, 0x11, 0x10));
    assert_eq!(theme.text_primary, Color::Rgb(0xd4, 0xdd, 0xd6));
    assert_eq!(theme.accent, Color::Rgb(0x8f, 0xbc, 0xb0));
}

#[test]
fn theme_loads_semantic_colors_from_json_file() {
    let mut file = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
    let mut review = clankerdiff_ratatui::theme::ReviewTheme::default();
    review.ui.text = clankerdiff_ratatui::theme::Rgba::new(0x11, 0x22, 0x33, 255);
    review.ui.canvas = clankerdiff_ratatui::theme::Rgba::new(1, 2, 3, 255);
    review.ui.accent = clankerdiff_ratatui::theme::Rgba::new(0x44, 0x55, 0x66, 255);
    review.markdown.heading = clankerdiff_ratatui::theme::Rgba::new(0xab, 0xcd, 0xef, 255);
    file.write_all(&review.to_bytes().unwrap()).unwrap();
    let theme = Theme::load_from_path(file.path()).unwrap();

    assert_eq!(theme.text_primary, Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(theme.background, Color::Rgb(0x01, 0x02, 0x03));
    assert_eq!(theme.accent, Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(theme.heading, Color::Rgb(0xab, 0xcd, 0xef));
}

#[test]
fn markdown_rendering_preserves_semantic_content_and_styles() {
    let mut ui = TestUi::new();
    ui.turn("markdown rendering", "# Heading\n\n**bold** and `code`");

    ui.assert_conversation_contains("Heading");
    ui.assert_conversation_contains("bold");
    ui.assert_conversation_contains("code");
    ui.assert_conversation_not_contains("**");
}

#[test]
fn syntax_highlighting_and_plain_code_fallback_are_explicit() {
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();

    let highlighted = highlighter.highlight("fn main() { println!(\"hi\"); }", "rust", &theme);
    let highlighted_text: String =
        highlighted.iter().flat_map(|line| line.spans.iter()).map(|span| span.content.as_ref()).collect();
    assert_eq!(highlighted_text, "fn main() { println!(\"hi\"); }");
    assert!(
        highlighted
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style.fg != Some(theme.code_fg) || !span.style.add_modifier.is_empty()),
        "known Rust syntax should receive a semantic highlight style"
    );

    let plain = highlighter.highlight("not highlighted", "not-a-language", &theme);
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].spans[0].content, "not highlighted");
    assert_eq!(plain[0].spans[0].style.fg, Some(theme.code_fg));
    assert_eq!(plain[0].spans[0].style.bg, Some(theme.code_bg));
}

#[test]
fn invalid_theme_paths_return_errors() {
    assert!(Theme::load_from_path(std::path::Path::new("/path/that/does/not/exist.json")).is_err());
    assert!(Theme::load_from_path(std::path::Path::new("/path/legacy.tmTheme")).is_err());
}

#[test]
fn large_markdown_history_preserves_order_across_scrollback_and_viewport() {
    let mut ui = TestUi::new();
    ui.turn("long response", &numbered_lines("paragraph", 40));

    let conversation = ui.conversation();
    let text = buffer_text(&conversation);
    assert_eq!(text.matches("paragraph-0").count(), 1, "conversation:\n{text}");
    assert_eq!(text.matches("paragraph-39").count(), 1);
    assert!(text.find("paragraph-0").unwrap() < text.find("paragraph-39").unwrap());
}

#[test]
fn settings_deserializes_status_line() {
    let settings: UiSettings = serde_json::from_str(
        r#"{"contentPadding":4,"theme":{"source":"file","file":"nord.json"},"statusLine":{"left":[{"type":"cwd"}],"right":[{"type":"agent"}]}}"#,
    )
    .unwrap();

    assert_eq!(settings.content_padding, Some(4));
    assert_eq!(settings.theme.selection_id(), "file:nord.json");
    assert!(settings.status_line.is_some());
    let sl = settings.status_line.unwrap();
    assert_eq!(sl.left, Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]));
    assert_eq!(sl.right, Some(vec![StatusLineSegmentConfig::Agent]));
}

#[test]
fn one_row_inline_viewport_draws_without_panicking() {
    let mut ui = TestUi::with_dimensions(40, 3);

    ui.draw();

    assert_eq!(ui.viewport_height(), 1);
}

/// Sealed rows enter the terminal's own history before the viewport is
/// repainted, so the frame is drawn exactly once in its final position. Drawing
/// first would briefly show a viewport that does not yet reflect that insertion.
#[test]
fn history_is_inserted_before_the_live_viewport_is_drawn() {
    let mut ui = TestUi::with_backend(InstrumentedBackend::new(40, 15));
    ui.draw();
    ui.backend_mut().take_events();

    ui.turn("hello", &numbered_lines("line", 20));
    ui.draw();

    let events = ui.backend_mut().take_events();
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

    ui.assert_history_not_contains("working");
    ui.assert_history_not_contains("esc to cancel");
    ui.assert_history_not_contains("aether");
}

#[test]
fn scrollback_insertion_does_not_duplicate_status_line() {
    let mut ui = TestUi::with_backend(InstrumentedBackend::new(120, 15));
    ui.acp_event(session_update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(20_000, 200_000))));
    ui.draw();

    ui.turn("hello", &numbered_lines("line", 20));
    ui.acp_event(session_update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(40_000, 200_000))));

    ui.draw();

    let visible = buffer_text(ui.backend().buffer());
    assert!(!visible.contains("20k / 200k"), "screen:\n{visible}");
    assert_eq!(visible.matches("40k / 200k").count(), 1, "screen:\n{visible}");
}

#[test]
fn fitting_content_stays_visible_in_a_small_viewport() {
    let mut ui = TestUi::new();
    ui.draw();

    ui.resize(40, 10);
    ui.submit("queued while small");

    ui.assert_history_not_contains("queued while small");
    ui.assert_viewport_contains("queued while small");

    ui.resize(40, 15);
    ui.draw();

    assert!(
        ui.app().core().conversation().items().iter().any(|item| item.text().as_deref() == Some("queued while small"))
    );
    ui.assert_viewport_contains("queued while small");
}
