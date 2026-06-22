use tui::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_ctrl_c_exits_while_file_picker_is_open() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('@'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;
    assert!(has_file_picker(renderer.writer()), "File picker should be open after typing @");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;

    assert!(matches!(action, LoopAction::Exit));
    Ok(())
}

#[tokio::test]
async fn test_space_closes_file_picker_without_selection() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('@'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;
    assert!(has_file_picker(renderer.writer()), "File picker should be open");

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;

    assert!(!has_file_picker(renderer.writer()), "File picker should be closed");
    Ok(())
}
