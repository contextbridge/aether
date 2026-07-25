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

// ── File mention selection ───────────────────────────────────────

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
