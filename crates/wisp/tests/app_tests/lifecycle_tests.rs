use tui::testing::TestTerminal;
use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_connection_closed_exits() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let action = renderer.on_connection_closed().unwrap();
    assert!(matches!(action, LoopAction::Exit));
}

#[tokio::test]
async fn test_ctrl_c_emits_exit() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).await.unwrap();
    assert!(matches!(action, LoopAction::Continue), "first Ctrl-C should not exit");

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).await.unwrap();
    assert!(matches!(action, LoopAction::Exit), "second Ctrl-C should exit");
}

#[tokio::test]
async fn test_escape_while_waiting_emits_cancel() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    // Submit a prompt to enter waiting state
    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    // Press Escape while waiting — should cancel
    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await.unwrap();

    assert!(matches!(action, LoopAction::Continue));
}

#[tokio::test]
async fn test_escape_while_not_waiting_does_nothing() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await.unwrap();

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Escape should be a no-op when not waiting");
}
