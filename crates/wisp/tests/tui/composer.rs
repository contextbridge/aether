use super::support::*;

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

    // The text fills both rows exactly, so the cursor wraps onto a third.
    assert_eq!(layout.lines.len(), 5, "wrapped: top rule, 3 content rows, bottom rule");
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
fn narrow_unicode_wrap_preserves_characters_and_display_width() {
    let mut composer = Composer::new();
    composer.insert_str("界界界");

    let layout = composer.layout(5, &Theme::default());
    let content = &layout.lines[1..layout.lines.len() - 1];
    let rendered: String = content.iter().map(line_text).collect();

    assert_eq!(rendered.matches('界').count(), 3);
    assert!(content.iter().all(|line| line.width() <= 5), "wrapped row exceeds terminal width: {rendered:?}");
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
fn cursor_wraps_onto_its_own_row_when_the_last_row_is_full() {
    let mut composer = Composer::new();
    composer.insert_str("abcd");

    let layout = composer.layout(6, &Theme::default());

    assert_eq!(layout.lines.len(), 4, "top rule, the full row, the row the cursor wrapped onto, bottom rule");
    assert_eq!(layout.cursor, Position::new(2, 2), "the cursor belongs at the start of the row below the full one");
}

#[test]
fn vertical_movement_follows_wrapped_rows() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefgh");
    composer.move_left();
    composer.move_left();
    composer.on_resize(6);

    assert!(composer.move_up(), "up should step onto the wrapped row above rather than recall history");
    assert_eq!(composer.cursor_position(), (0, 2));
    assert!(composer.move_down(), "down should step back onto the wrapped row below");
    assert_eq!(composer.cursor_position(), (0, 6));
}

#[test]
fn vertical_movement_stops_at_the_first_and_last_wrapped_rows() {
    let mut composer = Composer::new();
    composer.insert_str("abcdefgh");
    composer.on_resize(6);

    assert!(!composer.move_down(), "the cursor is already on the last row, so history recall takes over");
    while composer.move_up() {}
    assert_eq!(composer.cursor_position(), (0, 0));
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

#[test]
fn tiny_terminal_does_not_overwrite_status_line() {
    let mut ui = TestUi::new();
    ui.type_text("hello");

    ui.draw();
    let viewport = ui.viewport_text();

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
    let mut app = make_app();
    app.type_text("hello");
    app.key(key(KeyCode::Left));
    app.key(key(KeyCode::Left));
    app.key(ctrl('a'));
    assert_eq!(app.app().composer().cursor_position(), (0, 0));
}

#[test]
fn app_routes_ctrl_e_to_move_line_end() {
    let mut app = make_app();
    app.type_text("hello");
    app.key(key(KeyCode::Home));
    app.key(ctrl('e'));
    assert_eq!(app.app().composer().cursor_position(), (0, 5));
}

#[test]
fn shift_enter_closes_command_overlay_and_inserts_newline() {
    let mut app = make_app();
    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion(), "command overlay active");

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(!app.app().composer().has_completion(), "overlay closed after Shift+Enter");
    assert_eq!(app.app().composer().text(), "/\n");
}

#[test]
fn ctrl_j_closes_file_overlay_and_inserts_newline() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = make_app_in(directory.path().to_path_buf());
    app.key(key(KeyCode::Char('@')));
    assert!(app.app().composer().has_completion(), "file overlay active");

    app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    assert!(!app.app().composer().has_completion(), "overlay closed after Ctrl-J");
    assert_eq!(app.app().composer().text(), "@\n");
}

#[test]
fn alt_enter_closes_command_overlay_and_inserts_newline() {
    let mut app = make_app();
    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion());

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    assert!(!app.app().composer().has_completion(), "overlay closed after Alt+Enter");
    assert_eq!(app.app().composer().text(), "/\n");
}

#[test]
fn clear_replaces_the_conversation_once() {
    let mut app = make_app();
    let old_conversation = app.app().conversation_id();

    app.type_text("/clear");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_ne!(app.app().conversation_id(), old_conversation);
    assert_eq!(
        app.app()
            .conversation_items()
            .iter()
            .filter(|item| matches!(item.content(), ConversationContent::Notice(_)))
            .count(),
        0,
    );
}

#[test]
fn composer_highlights_at_mentions_in_info_color() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("lib.rs"), "old\n").unwrap();

    let theme = Theme::default();
    let mut composer = Composer::new();
    composer.insert_str("fix ");
    composer.insert_char('@');
    open_file_picker(&mut composer, root);
    composer.accept_file();
    // buffer is now "fix @lib.rs "

    let layout = composer.layout(80, &theme);
    let prompt_line = &layout.lines[1];
    let mention_span = prompt_line
        .spans
        .iter()
        .find(|span| span.content.contains("@lib.rs"))
        .expect("a span containing the mention should exist");
    assert_eq!(mention_span.style.fg, Some(theme.info), "@mention should be highlighted in the info colour");

    let plain_span = prompt_line.spans.iter().find(|span| span.content.contains("fix")).expect("plain text span");
    assert_eq!(plain_span.style.fg, Some(theme.text_primary), "non-mention text stays primary");
}

#[test]
fn selected_mentions_match_whole_tokens_not_substrings() {
    // A mention whose display name is a prefix of another token must not be treated as
    // active. Previously `@foo.rs` would also match the text `@foo.rsx` via `contains`.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("foo.rs"), "old\n").unwrap();

    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, root);
    composer.accept_file();
    assert_eq!(composer.text(), "@foo.rs ");

    // Extend the accepted token into a different name: `@foo.rsx`.
    composer.move_line_end();
    composer.backspace();
    composer.insert_char('x');
    assert_eq!(composer.text(), "@foo.rsx");

    assert!(
        composer.selected_mentions().is_empty(),
        "prefix mention should not be selected for a longer token: {:?}",
        composer.selected_mentions()
    );
}

#[test]
fn selected_matches_mention_with_exact_token() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("lib.rs"), "old\n").unwrap();

    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, root);
    composer.accept_file();

    let mentions = composer.selected_mentions();
    assert_eq!(mentions.len(), 1, "exact mention token should be selected");
    assert_eq!(mentions[0].display_name, "lib.rs");
}

#[test]
fn file_picker_completes_from_the_fake_filesystem() {
    let mut ui = TestUiBuilder::new().working_dir("/workspace").build();
    ui.executor_mut().filesystem_mut().write_file("/workspace/src/main.rs", b"fn main() {}");
    ui.executor_mut().filesystem_mut().write_file("/workspace/readme.md", b"# readme");

    ui.type_text("@");
    ui.settle_tasks();
    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("readme.md"), "picker should list fake files:\n{viewport}");
    assert!(viewport.contains("src/main.rs"), "picker should list nested fake files:\n{viewport}");
}
