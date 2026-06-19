use tui::testing::TestTerminal;
use tui::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_ctrl_c_exits_while_file_picker_is_open() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('@'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();
    assert!(has_file_picker(renderer.writer()), "File picker should be open after typing @");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();

    assert!(matches!(action, LoopAction::Exit));
}

#[tokio::test]
async fn test_space_closes_file_picker_without_selection() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('@'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();
    assert!(has_file_picker(renderer.writer()), "File picker should be open");

    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();

    assert!(!has_file_picker(renderer.writer()), "File picker should be closed");
}
