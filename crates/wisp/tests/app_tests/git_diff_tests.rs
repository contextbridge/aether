use tui::{KeyCode, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn ctrl_g_toggles_git_diff_and_mouse_capture() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    assert!(!renderer.needs_mouse_capture());

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(renderer.needs_mouse_capture(), "git diff mode should capture mouse input");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(!renderer.needs_mouse_capture(), "closing git diff should release mouse capture");
    Ok(())
}

#[tokio::test]
async fn ctrl_g_is_ignored_while_modal_is_open() -> TestResult {
    let mut renderer = open_settings(&make_settings_options(), (TEST_WIDTH, 40)).await?;
    assert!(has_settings_menu(renderer.writer()), "settings menu should be visible");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;

    assert!(has_settings_menu(renderer.writer()), "settings menu should remain visible");
    assert!(renderer.needs_mouse_capture(), "modal should continue capturing mouse input");
    Ok(())
}

#[tokio::test]
async fn esc_in_git_diff_does_not_cancel_waiting_prompt() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "hello").await?;
    press(&mut renderer, Enter).await?;
    assert_buffer_contains(renderer.writer(), "esc to interrupt");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(renderer.needs_mouse_capture(), "git diff should be active");

    press(&mut renderer, Esc).await?;

    assert!(!renderer.needs_mouse_capture(), "Esc in git diff should close diff mode");
    assert_buffer_contains(renderer.writer(), "esc to interrupt");
    Ok(())
}
