use super::support::*;

fn make_app_with_prompt_search() -> (App, UnboundedReceiver<PromptCommand>) {
    AppBuilder::new().prompt_search().build()
}

fn make_ui_with_prompt_search() -> TestUi {
    TestUiBuilder::new().prompt_search().build()
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
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));

    let cmd = ui.command_rx().try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::SearchPrompts(params) if params.query == "h"),
        "expected SearchPrompts with query 'h', got {cmd:?}"
    );

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("history search: h"), "viewport:\n{viewport}");
    assert!(viewport.contains("searching"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_empty_query_renders_instruction() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("history search:"), "viewport:\n{viewport}");
    assert!(viewport.contains("type to search prompt history"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_shows_results_after_response() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello world", 0, 1)],
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("hello world"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_no_results_shows_no_matches() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.type_text("zzz");
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen("zzz", vec![], 3)));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("no matching prompts"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_shows_error_on_failure() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchFailed {
        query: "h".to_string(),
        search_generation: 1,
        error: "connection refused".to_string(),
    });

    ui.draw();
    let viewport = ui.viewport_text();
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
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "h",
        vec![prompt_search_result("hello", 0, 1), prompt_search_result("hey", 0, 1)],
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("hello"), "viewport:\n{viewport}");

    ui.key(key(KeyCode::Down));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("hey"), "viewport:\n{viewport}");

    ui.key(key(KeyCode::Up));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("hello"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_stale_response_is_ignored() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));
    ui.key(key(KeyCode::Char('e')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "he",
        vec![prompt_search_result("hello", 0, 2)],
        2,
    )));

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "h",
        vec![prompt_search_result("STALE", 0, 1)],
        1,
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("hello"), "should show current result:\n{viewport}");
    assert!(!viewport.contains("STALE"), "should not show stale result:\n{viewport}");
}

#[test]
fn prompt_search_prefills_selected_result_with_cursor_at_match() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('q')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response(
        "q",
        vec![prompt_search_result("the quick brown fox", 4, 9)],
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("the quick brown fox"), "viewport:\n{viewport}");

    let (row, col) = ui.app().composer().cursor_position();
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
    let mut ui = make_ui_with_prompt_search();
    ui.type_text("draft");
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('h')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response("h", vec![prompt_search_result("hello", 0, 1)])));

    ui.key(key(KeyCode::Backspace));

    assert!(ui.app().composer().has_prompt_search());
    assert_eq!(ui.app().composer().text(), "draft");

    ui.draw();
    let viewport = ui.viewport_text();
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
    assert!(app.composer().has_completion());

    app.on_key(ctrl('r'));
    assert!(!app.composer().has_prompt_search());
    assert!(app.composer().has_completion());
}

#[test]
fn prompt_search_unicode_query_is_accepted() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('ñ')));

    let cmd = ui.command_rx().try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::SearchPrompts(params) if params.query == "ñ"),
        "expected unicode query 'ñ', got {cmd:?}"
    );

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("history search: ñ"), "viewport:\n{viewport}");
}

#[test]
fn prompt_search_rows_truncate_prompt_and_show_cwd_basename() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.type_text("quick");
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "quick",
        vec![prompt_search_result_with_cwd(
            "the quick brown fox jumps over the lazy dog",
            4,
            9,
            std::path::PathBuf::from("/some/deeply/nested/project/repo-name"),
        )],
        5,
    )));

    ui.draw();
    let viewport = ui.viewport_text();
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
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));

    // First search for "xy"
    ui.key(key(KeyCode::Char('x')));
    ui.key(key(KeyCode::Char('y')));
    let _ = ui.command_rx().try_recv().unwrap();
    let _ = ui.command_rx().try_recv().unwrap();

    // Backspace twice to get empty query (draft restored)
    ui.key(key(KeyCode::Backspace));
    ui.key(key(KeyCode::Backspace));

    // Type "xy" again — same query string but generation is now higher
    ui.key(key(KeyCode::Char('x')));
    ui.key(key(KeyCode::Char('y')));
    let _ = ui.command_rx().try_recv().unwrap();
    let _ = ui.command_rx().try_recv().unwrap();

    // Stale response from first "xy" with generation=2 should be ignored
    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("STALE_FIRST", 0, 2)],
        2,
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("STALE_FIRST"), "stale response from first 'xy' must be ignored:\n{viewport}");
    assert!(viewport.contains("searching"), "second 'xy' should still be loading:\n{viewport}");

    // Fresh response from second "xy" with generation=5 should be accepted
    // (gen: 0→x=1, xy=2, backspace=3, backspace(empty)=3, x=4, xy=5)
    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("FRESH_SECOND", 0, 2)],
        5,
    )));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("FRESH_SECOND"), "fresh response from second 'xy' must be shown:\n{viewport}");
}

#[test]
fn prompt_search_send_failure_is_visible_in_picker() {
    let (mut ui, fail_signal) = TestUiBuilder::new().prompt_search().build_failable();
    ui.key(ctrl('r'));

    fail_signal.store(true, Ordering::Relaxed);
    ui.key(key(KeyCode::Char('h')));
    assert!(ui.command_rx().try_recv().is_err(), "send should have failed");

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("search failed"), "send failure must be visible in picker:\n{viewport}");

    assert!(ui.app().composer().has_prompt_search(), "picker must remain open");
    assert!(!ui.app().exit_requested(), "app must remain interactive");
}

#[test]
fn prompt_search_stale_failure_is_accepted_for_current_query() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('x')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchFailed {
        query: "x".to_string(),
        search_generation: 1,
        error: "server error".to_string(),
    });

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("server error"), "failure must be visible:\n{viewport}");

    assert!(ui.app().composer().has_prompt_search(), "picker must remain open after failure");
}

#[test]
fn prompt_search_stale_failure_must_not_overwrite_newer_success() {
    let mut ui = make_ui_with_prompt_search();
    ui.key(ctrl('r'));
    ui.key(key(KeyCode::Char('x')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.key(key(KeyCode::Char('y')));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(AcpEvent::PromptSearchResults(prompt_search_response_gen(
        "xy",
        vec![prompt_search_result("fresh result for xy", 0, 2)],
        2,
    )));

    ui.acp_event(AcpEvent::PromptSearchFailed {
        query: "x".to_string(),
        search_generation: 1,
        error: "stale error".to_string(),
    });

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("fresh result for xy"), "newer success results must survive stale failure:\n{viewport}");
    assert!(!viewport.contains("stale error"), "stale failure must be ignored:\n{viewport}");
}
