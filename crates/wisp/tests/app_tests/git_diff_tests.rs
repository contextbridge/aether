use tui::testing::TestTerminal;
use tui::{KeyCode, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn ctrl_g_toggles_git_diff_and_mouse_capture() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    assert!(!renderer.needs_mouse_capture());

    send_key(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
    assert!(renderer.needs_mouse_capture(), "git diff mode should capture mouse input");

    send_key(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
    assert!(!renderer.needs_mouse_capture(), "closing git diff should release mouse capture");
}

#[tokio::test]
async fn ctrl_g_is_ignored_while_modal_is_open() {
    let mut renderer = open_settings(&make_settings_options(), (TEST_WIDTH, 40)).await;
    assert!(has_settings_menu(renderer.writer()), "settings menu should be visible");

    send_key(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await;

    assert!(has_settings_menu(renderer.writer()), "settings menu should remain visible");
    assert!(renderer.needs_mouse_capture(), "modal should continue capturing mouse input");
}

#[tokio::test]
async fn esc_in_git_diff_does_not_cancel_waiting_prompt() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;
    press_enter(&mut renderer).await;
    assert_buffer_contains(renderer.writer(), "esc to interrupt");

    send_key(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
    assert!(renderer.needs_mouse_capture(), "git diff should be active");

    press_esc(&mut renderer).await;

    assert!(!renderer.needs_mouse_capture(), "Esc in git diff should close diff mode");
    assert_buffer_contains(renderer.writer(), "esc to interrupt");
}
