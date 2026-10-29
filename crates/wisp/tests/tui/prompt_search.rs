use super::support::*;

#[test]
fn remote_prompt_search_keeps_server_paths_and_accepts_results() {
    let cwd = std::env::var_os("HOME").map_or_else(|| "/server".into(), std::path::PathBuf::from);
    let mut ui =
        TestUiBuilder::new().session(|session| session.remote_workspace().prompt_search()).dimensions(160, 24).build();
    search(&mut ui, "h", vec![PromptSearchResult { cwd: cwd.clone(), ..prompt_search_hit("hello world", 0, 1) }]);
    ui.assert_viewport_contains(&cwd.display().to_string());
    ui.key(key(KeyCode::Enter));
    assert_eq!(ui.app().composer().text(), "hello world");
    ui.key(key(KeyCode::Enter));
    assert_eq!(prompt_text(&sent_prompt(&mut ui)), Some("hello world"));
}

#[test]
fn ctrl_r_is_noop_when_prompt_search_capability_is_missing() {
    let mut app = TestUi::new();
    app.type_text("draft");
    app.key(ctrl('r'));
    assert_eq!(app.app().composer().text(), "draft");
}

#[test]
fn ctrl_r_opens_prompt_search_when_capability_is_enabled() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("draft");
    app.key(ctrl('r'));
    assert!(app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "draft");
}

#[test]
fn prompt_search_shows_loading_state_after_query() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    start_search(&mut ui, "h");

    ui.assert_viewport_contains("history search: h");
    ui.assert_viewport_contains("searching");
}

#[test]
fn prompt_search_empty_query_renders_instruction() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    ui.key(ctrl('r'));

    ui.assert_viewport_contains("history search:");
    ui.assert_viewport_contains("type to search prompt history");
}

#[test]
fn prompt_search_shows_results_after_response() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut ui, "h", vec![prompt_search_hit("hello world", 0, 1)]);

    ui.assert_viewport_contains("hello world");
}

#[test]
fn prompt_search_no_results_shows_no_matches() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut ui, "zzz", vec![]);

    ui.assert_viewport_contains("no matching prompts");
}

#[test]
fn prompt_search_failure_is_shown_in_the_open_picker() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).dimensions(80, 15).build();
    start_search(&mut ui, "h");
    answer_search(&mut ui, "h", Err(agent_error("connection refused")));

    ui.assert_viewport_contains("error: ACP protocol error: connection refused");
    assert!(ui.app().composer().has_prompt_search(), "picker must remain open");
    assert!(!ui.app().exit_requested(), "app must remain interactive");
}

#[test]
fn prompt_search_enter_confirms_and_inserts_result() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("draft");
    search(&mut app, "h", vec![prompt_search_hit("hello world", 0, 5)]);

    app.key(key(KeyCode::Enter));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "hello world");
}

#[test]
fn prompt_search_enter_without_selection_restores_draft() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("draft");
    search(&mut app, "zzz", vec![]);

    app.key(key(KeyCode::Enter));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "draft");
}

#[test]
fn prompt_search_escape_restores_draft() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("original draft");
    search(&mut app, "h", vec![prompt_search_hit("hello world", 0, 5)]);

    app.key(key(KeyCode::Esc));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "original draft");
}

#[test]
fn prompt_search_escape_restores_multiline_draft() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("line1");
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.type_text("line2");

    app.key(ctrl('r'));
    app.key(key(KeyCode::Esc));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "line1\nline2");
}

#[test]
fn prompt_search_up_and_down_preview_the_selected_result() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut ui, "h", vec![prompt_search_hit("hello", 0, 1), prompt_search_hit("hey", 0, 1)]);
    assert_eq!(ui.app().composer().text(), "hello");

    ui.key(key(KeyCode::Down));
    assert_eq!(ui.app().composer().text(), "hey");

    ui.key(key(KeyCode::Up));
    assert_eq!(ui.app().composer().text(), "hello");
}

#[test]
fn prompt_search_result_replacement_resets_selection_to_first() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut app, "h", vec![prompt_search_hit("first result", 0, 1), prompt_search_hit("second result", 0, 1)]);
    app.key(key(KeyCode::Down));
    app.key(key(KeyCode::Backspace));
    app.type_text("h");

    answer_search(
        &mut app,
        "h",
        Ok(prompt_search_response(
            "h",
            vec![prompt_search_hit("replacement first", 0, 1), prompt_search_hit("replacement second", 0, 1)],
        )),
    );
    app.key(key(KeyCode::Enter));

    assert_eq!(app.app().composer().text(), "replacement first");
}

#[test]
fn prompt_search_prefills_selected_result_with_cursor_at_match() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut ui, "q", vec![prompt_search_hit("the quick brown fox", 4, 9)]);

    ui.assert_viewport_contains("the quick brown fox");

    let (row, col) = ui.app().composer().cursor_position();
    assert_eq!((row, col), (0, 9), "cursor should be at match end position 9");
}

#[test]
fn prompt_search_paste_sanitizes_query() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.key(ctrl('r'));

    app.paste("hello\nworld");

    assert_eq!(app.agent().search.try_recv().unwrap().query, "helloworld");
}

#[test]
fn prompt_search_backspace_to_empty_restores_draft_but_keeps_picker_open() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    ui.type_text("draft");
    search(&mut ui, "h", vec![prompt_search_hit("hello", 0, 1)]);

    ui.key(key(KeyCode::Backspace));

    assert!(ui.app().composer().has_prompt_search());
    assert_eq!(ui.app().composer().text(), "draft");

    ui.assert_viewport_contains("type to search prompt history");
}

#[test]
fn prompt_search_ctrl_r_does_not_steal_from_modal() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();

    app.elicit(form_elicitation("test", "", ElicitationSchema::new()));

    assert!(app.app().has_modal());
    app.key(ctrl('r'));
    assert!(!app.app().composer().has_prompt_search());
}

#[test]
fn prompt_search_ctrl_r_does_not_open_during_composer_overlay() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion());

    app.key(ctrl('r'));
    assert!(!app.app().composer().has_prompt_search());
    assert!(app.app().composer().has_completion());
}

#[test]
fn prompt_search_unicode_query_is_accepted() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    start_search(&mut ui, "ñ");

    ui.assert_viewport_contains("history search: ñ");
}

#[test]
fn prompt_search_rows_truncate_prompt_and_show_cwd_basename() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(
        &mut ui,
        "quick",
        vec![PromptSearchResult {
            cwd: "/some/deeply/nested/project/repo-name".into(),
            ..prompt_search_hit("the quick brown fox jumps over the lazy dog", 4, 9)
        }],
    );

    ui.assert_viewport_contains("…");
    ui.assert_viewport_contains("repo-name");
}

#[test]
fn prompt_search_query_editing_triggers_multiple_searches() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.key(ctrl('r'));
    app.type_text("ab");

    let queries: Vec<_> =
        std::iter::from_fn(|| app.agent().search.try_recv().ok()).map(|params| params.query).collect();
    assert_eq!(queries, ["a", "ab"]);
}

#[test]
fn prompt_search_enter_preserves_cursor_at_match_end() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    app.type_text("draft");
    search(&mut app, "q", vec![prompt_search_hit("the quick brown fox", 4, 9)]);

    app.key(key(KeyCode::Enter));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "the quick brown fox");
    let (row, col) = app.app().composer().cursor_position();
    assert_eq!((row, col), (0, 9), "cursor must be at match end (9), not end of prompt");
}

#[test]
fn prompt_search_enter_preserves_cursor_after_manual_navigation() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    search(&mut app, "h", vec![prompt_search_hit("hello", 0, 1), prompt_search_hit("hi there", 0, 1)]);

    app.key(key(KeyCode::Down));
    app.key(key(KeyCode::Enter));

    assert!(!app.app().composer().has_prompt_search());
    assert_eq!(app.app().composer().text(), "hi there");
    let (row, col) = app.app().composer().cursor_position();
    assert_eq!((row, col), (0, 1), "cursor must be at match end (1) for 'hi there'");
}

#[test]
fn prompt_search_identical_repeated_query_accepts_any_matching_response() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::prompt_search).build();
    ui.key(ctrl('r'));

    // Search for "xy", clear it, and type the identical query again. A response
    // for "xy" answers both searches identically, so whichever issuance
    // produced it, it is current for the query on screen.
    ui.type_text("xy");
    ui.key(key(KeyCode::Backspace));
    ui.key(key(KeyCode::Backspace));
    ui.type_text("xy");

    answer_search(&mut ui, "xy", Ok(prompt_search_response("xy", vec![prompt_search_hit("xy history hit", 0, 2)])));

    ui.assert_viewport_contains("xy history hit");
}
