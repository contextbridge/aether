use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_connection_closed_exits() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let action = renderer.on_connection_closed()?;
    assert!(matches!(action, LoopAction::Exit));
    Ok(())
}

#[tokio::test]
async fn test_ctrl_c_emits_exit() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).await?;
    assert!(matches!(action, LoopAction::Continue), "first Ctrl-C should not exit");

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).await?;
    assert!(matches!(action, LoopAction::Exit), "second Ctrl-C should exit");
    Ok(())
}

#[tokio::test]
async fn test_escape_while_waiting_emits_cancel() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    // Submit a prompt to enter waiting state
    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    // Press Escape while waiting — should cancel
    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await?;

    assert!(matches!(action, LoopAction::Continue));
    Ok(())
}

#[tokio::test]
async fn test_escape_while_not_waiting_does_nothing() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await?;

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Escape should be a no-op when not waiting");
    Ok(())
}
