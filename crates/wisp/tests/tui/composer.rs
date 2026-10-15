use super::support::*;

#[test]
fn layout_frames_wrapped_rows_between_rules_and_tracks_the_cursor() {
    let cases = [
        ("", 0, 80, 1, (2, 1)),
        ("abc", 0, 80, 1, (5, 1)),
        ("a🎉界", 0, 80, 1, (7, 1)),
        ("@main.rs @lib.rs", 0, 80, 1, (18, 1)),
        ("abcdefgh", 2, 80, 1, (8, 1)),
        ("abcdefgh", 2, 6, 2, (4, 2)),
        ("hello world", 5, 9, 2, (8, 1)),
        ("abcd", 0, 6, 2, (2, 2)),
        ("abcdefghijkl", 0, 8, 3, (2, 3)),
    ];
    for (text, moves_left, width, rows, cursor) in cases {
        let mut composer = Composer::new();
        composer.insert_str(text);
        for _ in 0..moves_left {
            composer.move_left();
        }
        let layout = composer.layout(width, &Theme::default());
        let lines: Vec<String> = layout.lines.iter().map(line_text).collect();
        let rule = "─".repeat(usize::from(width));
        assert!(lines.first() == Some(&rule) && lines.last() == Some(&rule), "{text:?} at {width}: {lines:?}");
        assert_eq!(lines.len() - 2, rows, "{text:?} at {width}: {lines:?}");
        assert_eq!((layout.cursor.x, layout.cursor.y), cursor, "{text:?} at {width}");
    }
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
fn tiny_terminal_does_not_overwrite_status_line() {
    let mut ui = TestUi::new();
    ui.type_text("hello");

    ui.assert_viewport_contains("aether");
    ui.assert_viewport_contains("> hello");
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
fn paste_keeps_text_newlines_and_tabs_but_strips_other_control_characters() {
    for (pasted, kept) in [
        ("abc\x01def\x02ghi", "abcdefghi"),
        ("abc\r\x08\x7fdef", "abcdef"),
        ("line one\nline two", "line one\nline two"),
        ("col1\tcol2", "col1\tcol2"),
        ("héllo 🎉 wörld", "héllo 🎉 wörld"),
    ] {
        let mut composer = Composer::new();
        composer.insert_paste(pasted);
        assert_eq!(composer.text(), kept, "{pasted:?}");
    }
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
    let mut app = TestUi::new();
    app.type_text("hello");
    app.key(key(KeyCode::Left));
    app.key(key(KeyCode::Left));
    app.key(ctrl('a'));
    assert_eq!(app.app().composer().cursor_position(), (0, 0));
}

#[test]
fn app_routes_ctrl_e_to_move_line_end() {
    let mut app = TestUi::new();
    app.type_text("hello");
    app.key(key(KeyCode::Home));
    app.key(ctrl('e'));
    assert_eq!(app.app().composer().cursor_position(), (0, 5));
}

#[test]
fn shift_enter_closes_command_overlay_and_inserts_newline() {
    let mut app = TestUi::new();
    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion(), "command overlay active");

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(!app.app().composer().has_completion(), "overlay closed after Shift+Enter");
    assert_eq!(app.app().composer().text(), "/\n");
}

#[test]
fn ctrl_j_closes_file_overlay_and_inserts_newline() {
    let mut app = TestUiBuilder::new().working_dir("/workspace").build();
    app.key(key(KeyCode::Char('@')));
    assert!(app.app().composer().has_completion(), "file overlay active");

    app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    assert!(!app.app().composer().has_completion(), "overlay closed after Ctrl-J");
    assert_eq!(app.app().composer().text(), "@\n");
}

#[test]
fn alt_enter_closes_command_overlay_and_inserts_newline() {
    let mut app = TestUi::new();
    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion());

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    assert!(!app.app().composer().has_completion(), "overlay closed after Alt+Enter");
    assert_eq!(app.app().composer().text(), "/\n");
}

#[test]
fn clear_replaces_the_conversation_once() {
    let mut app = TestUi::new();
    let old_conversation = app.app().conversation_id();

    app.run_command("/clear");
    let _ = app.next_agent_command().unwrap();

    app.deliver_result(new_session_created("new-session", vec![select_option("model", "sonnet")]));

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
    let theme = Theme::default();
    let mut composer = Composer::new();
    composer.insert_str("fix ");
    composer.insert_char('@');
    open_file_picker(&mut composer, &["lib.rs"]);
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
    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, &["foo.rs"]);
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
    let mut composer = Composer::new();
    composer.insert_char('@');
    open_file_picker(&mut composer, &["lib.rs"]);
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

    ui.assert_viewport_contains("readme.md");
    ui.assert_viewport_contains("src/main.rs");
}
