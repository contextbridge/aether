use super::common::*;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult, prompt_search_capability};
use agent_client_protocol::schema as acp;
use std::path::PathBuf;
use tui::testing::{TestTerminal, assert_buffer_eq};
use tui::{KeyCode, KeyModifiers};

#[tokio::test]
async fn prompt_search_prefills_selected_history_result() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "hello").await;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)])).unwrap();

    let rule = "─".repeat(80);
    let mut expected = vec![rule.clone(), "> hello world".to_string(), rule, "history search: hello".to_string()];
    expected.push(format!("  hello world{}/tmp/repo", " ".repeat(58)));
    expected.push(expected_status_line(80, TEST_AGENT));
    assert_buffer_eq(renderer.writer(), &expected);
    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!((cursor_col, cursor_row), (7, 1));
}

#[tokio::test]
async fn prompt_search_restore_draft_on_escape() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "draft").await;
    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "hello").await;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)])).unwrap();
    press_esc(&mut renderer).await;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "draft", TEST_AGENT));
}

#[tokio::test]
async fn prompt_search_shows_backend_errors() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "hello").await;
    renderer.on_prompt_search_failed("hello", "boom").unwrap();

    assert_buffer_contains(renderer.writer(), "history search: hello");
    assert_buffer_contains(renderer.writer(), "error: boom");
}

#[tokio::test]
async fn ctrl_r_opens_prompt_search_when_capability_is_enabled() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "h").await;

    assert_buffer_contains(renderer.writer(), "history search: h");
    assert_buffer_contains(renderer.writer(), "searching…");
}

#[tokio::test]
async fn ctrl_r_is_noop_when_prompt_search_capability_is_missing() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "", TEST_AGENT));
}

#[tokio::test]
async fn prompt_search_enter_confirms_selected_result() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "hello").await;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)])).unwrap();
    press_enter(&mut renderer).await;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "hello world", TEST_AGENT));
}

#[tokio::test]
async fn stale_prompt_search_response_does_not_overwrite_current_selection() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "he").await;
    renderer.on_prompt_search_results(response("he", vec![result("hello world", 0, 2)])).unwrap();
    renderer.on_prompt_search_results(response("h", vec![result("OTHER", 0, 1)])).unwrap();

    assert_buffer_contains(renderer.writer(), "> hello world");
    assert_buffer_not_contains(renderer.writer(), "> OTHER");
}

#[tokio::test]
async fn prompt_search_empty_query_renders_instruction() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;

    let rule = "─".repeat(80);
    let expected = vec![
        rule.clone(),
        ">".to_string(),
        rule,
        "history search:".to_string(),
        "  type to search prompt history".to_string(),
        expected_status_line(80, TEST_AGENT),
    ];
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn prompt_search_empty_results_render_no_matches() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "draft").await;
    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "zzz").await;
    renderer.on_prompt_search_results(response("zzz", vec![])).unwrap();

    let rule = "─".repeat(80);
    let expected = vec![
        rule.clone(),
        "> draft".to_string(),
        rule,
        "history search: zzz".to_string(),
        "  no matching prompts".to_string(),
        expected_status_line(80, TEST_AGENT),
    ];
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn prompt_search_enter_without_selection_restores_draft() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "draft").await;
    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "zzz").await;
    renderer.on_prompt_search_results(response("zzz", vec![])).unwrap();
    press_enter(&mut renderer).await;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "draft", TEST_AGENT));
}

#[tokio::test]
async fn prompt_search_paste_sanitizes_query() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    renderer.on_paste("hello\nworld").await.unwrap();

    assert_buffer_contains(renderer.writer(), "history search: helloworld");
    assert_buffer_contains(renderer.writer(), "searching…");
}

#[tokio::test]
async fn prompt_search_backspace_to_empty_restores_draft_but_keeps_picker_open() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "draft").await;
    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "h").await;
    renderer.on_prompt_search_results(response("h", vec![result("hello", 0, 1)])).unwrap();
    press_backspace(&mut renderer).await;

    let rule = "─".repeat(80);
    let expected = vec![
        rule.clone(),
        "> draft".to_string(),
        rule,
        "history search:".to_string(),
        "  type to search prompt history".to_string(),
        expected_status_line(80, TEST_AGENT),
    ];
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn prompt_search_up_and_down_change_selected_prompt() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (80, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "h").await;
    renderer.on_prompt_search_results(response("h", vec![result("hello", 0, 1), result("hey", 0, 1)])).unwrap();
    assert_buffer_contains(renderer.writer(), "> hello");

    press_down(&mut renderer).await;
    assert_buffer_contains(renderer.writer(), "> hey");

    send_key(&mut renderer, KeyCode::Up, KeyModifiers::empty()).await;
    assert_buffer_contains(renderer.writer(), "> hello");
}

#[tokio::test]
async fn prompt_search_rows_truncate_prompt_and_show_cwd_basename() {
    let terminal = TestTerminal::new(40, 24);
    let mut renderer = Renderer::new_with_prompt_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        prompt_search_capabilities(),
        (40, 24),
    );
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "quick").await;
    renderer
        .on_prompt_search_results(response(
            "quick",
            vec![result_with_cwd(
                "the quick brown fox jumps over the lazy dog",
                4,
                9,
                PathBuf::from("/some/deeply/nested/project/repo-name"),
            )],
        ))
        .unwrap();

    assert_buffer_contains(renderer.writer(), "...");
    assert_buffer_contains(renderer.writer(), "repo-name");
}

fn prompt_search_capabilities() -> acp::PromptCapabilities {
    acp::PromptCapabilities::new().meta(Some(prompt_search_capability::to_meta()))
}

fn result(prompt: &str, start: usize, end: usize) -> PromptSearchResult {
    result_with_cwd(prompt, start, end, PathBuf::from("/tmp/repo"))
}

fn result_with_cwd(prompt: &str, start: usize, end: usize, cwd: PathBuf) -> PromptSearchResult {
    PromptSearchResult {
        session_id: "s1".to_string(),
        cwd,
        session_created_at: "2026-05-17T00:00:00Z".to_string(),
        prompt: prompt.to_string(),
        match_start: start,
        match_end: end,
    }
}

fn response(query: &str, results: Vec<PromptSearchResult>) -> PromptSearchResponse {
    PromptSearchResponse { query: query.to_string(), results, truncated: false }
}
