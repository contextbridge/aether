use super::common::*;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use std::path::PathBuf;
use tui::testing::assert_buffer_eq;
use tui::{KeyCode, KeyModifiers};

#[tokio::test]
async fn prompt_search_prefills_selected_history_result() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "hello").await?;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)]))?;

    let rule = "─".repeat(80);
    let mut expected = vec![rule.clone(), "> hello world".to_string(), rule, "history search: hello".to_string()];
    expected.push(format!("  hello world{}/tmp/repo", " ".repeat(58)));
    expected.push(expected_status_line(80, TEST_AGENT));
    assert_buffer_eq(renderer.writer(), &expected);
    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!((cursor_col, cursor_row), (7, 1));
    Ok(())
}

#[tokio::test]
async fn prompt_search_restore_draft_on_escape() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    type_string(&mut renderer, "draft").await?;
    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "hello").await?;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)]))?;
    press(&mut renderer, Esc).await?;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "draft", TEST_AGENT));
    Ok(())
}

#[tokio::test]
async fn prompt_search_shows_backend_errors() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "hello").await?;
    renderer.on_prompt_search_failed("hello", "boom")?;

    assert_buffer_contains(renderer.writer(), "history search: hello");
    assert_buffer_contains(renderer.writer(), "error: boom");
    Ok(())
}

#[tokio::test]
async fn ctrl_r_opens_prompt_search_when_capability_is_enabled() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "h").await?;

    assert_buffer_contains(renderer.writer(), "history search: h");
    assert_buffer_contains(renderer.writer(), "searching…");
    Ok(())
}

#[tokio::test]
async fn ctrl_r_is_noop_when_prompt_search_capability_is_missing() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "", TEST_AGENT));
    Ok(())
}

#[tokio::test]
async fn prompt_search_enter_confirms_selected_result() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "hello").await?;
    renderer.on_prompt_search_results(response("hello", vec![result("hello world", 0, 5)]))?;
    press(&mut renderer, Enter).await?;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "hello world", TEST_AGENT));
    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!((cursor_col, cursor_row), (13, 1));
    Ok(())
}

#[tokio::test]
async fn stale_prompt_search_response_does_not_overwrite_current_selection() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "he").await?;
    renderer.on_prompt_search_results(response("he", vec![result("hello world", 0, 2)]))?;
    renderer.on_prompt_search_results(response("h", vec![result("OTHER", 0, 1)]))?;

    assert_buffer_contains(renderer.writer(), "> hello world");
    assert_buffer_not_contains(renderer.writer(), "> OTHER");
    Ok(())
}

#[tokio::test]
async fn prompt_search_empty_query_renders_instruction() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;

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
    Ok(())
}

#[tokio::test]
async fn prompt_search_empty_results_render_no_matches() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    type_string(&mut renderer, "draft").await?;
    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "zzz").await?;
    renderer.on_prompt_search_results(response("zzz", vec![]))?;

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
    Ok(())
}

#[tokio::test]
async fn prompt_search_enter_without_selection_restores_draft() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    type_string(&mut renderer, "draft").await?;
    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "zzz").await?;
    renderer.on_prompt_search_results(response("zzz", vec![]))?;
    press(&mut renderer, Enter).await?;

    assert_buffer_eq(renderer.writer(), &expected_prompt(80, "draft", TEST_AGENT));
    Ok(())
}

#[tokio::test]
async fn prompt_search_paste_sanitizes_query() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    renderer.on_paste("hello\nworld").await?;

    assert_buffer_contains(renderer.writer(), "history search: helloworld");
    assert_buffer_contains(renderer.writer(), "searching…");
    Ok(())
}

#[tokio::test]
async fn prompt_search_backspace_to_empty_restores_draft_but_keeps_picker_open() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    type_string(&mut renderer, "draft").await?;
    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "h").await?;
    renderer.on_prompt_search_results(response("h", vec![result("hello", 0, 1)]))?;
    press(&mut renderer, Backspace).await?;

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
    Ok(())
}

#[tokio::test]
async fn prompt_search_up_and_down_change_selected_prompt() -> TestResult {
    let mut renderer = prompt_search_renderer((80, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "h").await?;
    renderer.on_prompt_search_results(response("h", vec![result("hello", 0, 1), result("hey", 0, 1)]))?;
    assert_buffer_contains(renderer.writer(), "> hello");

    press(&mut renderer, Down).await?;
    assert_buffer_contains(renderer.writer(), "> hey");

    press(&mut renderer, Up).await?;
    assert_buffer_contains(renderer.writer(), "> hello");
    Ok(())
}

#[tokio::test]
async fn prompt_search_rows_truncate_prompt_and_show_cwd_basename() -> TestResult {
    let mut renderer = prompt_search_renderer((40, 24))?;

    press_with_modifiers(&mut renderer, KeyCode::Char('r'), KeyModifiers::CONTROL).await?;
    type_string(&mut renderer, "quick").await?;
    renderer.on_prompt_search_results(response(
        "quick",
        vec![result_with_cwd(
            "the quick brown fox jumps over the lazy dog",
            4,
            9,
            PathBuf::from("/some/deeply/nested/project/repo-name"),
        )],
    ))?;

    assert_buffer_contains(renderer.writer(), "...");
    assert_buffer_contains(renderer.writer(), "repo-name");
    Ok(())
}

fn prompt_search_renderer(size: (u16, u16)) -> TestResult<Renderer> {
    RendererTest::new().size(size).session_capabilities(prompt_search_session_capabilities()).build()
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
