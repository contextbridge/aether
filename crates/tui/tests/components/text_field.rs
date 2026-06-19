use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui::Renderer;
use tui::TextField;

#[test]
fn empty_renders_cursor() {
    let mut tf = TextField::new(String::new());
    let term = render_component(|ctx| tf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["▏"]);
}

#[test]
fn with_value_renders_text_and_cursor() {
    let mut tf = TextField::new("hello".to_string());
    let term = render_component(|ctx| tf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["hello▏"]);
}

#[tokio::test]
async fn typing_appends_to_render() {
    let mut tf = TextField::new(String::new());
    tf.on_event(&Event::Key(key(KeyCode::Char('a')))).await;
    tf.on_event(&Event::Key(key(KeyCode::Char('b')))).await;
    tf.on_event(&Event::Key(key(KeyCode::Char('c')))).await;
    let term = render_component(|ctx| tf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["abc▏"]);
}

#[tokio::test]
async fn backspace_removes_from_render() {
    let mut tf = TextField::new("hi".to_string());
    tf.on_event(&Event::Key(key(KeyCode::Backspace))).await;
    let term = render_component(|ctx| tf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["h▏"]);
}

#[tokio::test]
async fn backspace_on_empty_renders_cursor() {
    let mut tf = TextField::new(String::new());
    tf.on_event(&Event::Key(key(KeyCode::Backspace))).await;
    let term = render_component(|ctx| tf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["▏"]);
}

#[tokio::test]
async fn option_left_escape_b_moves_backward_by_word() {
    let mut tf = TextField::new("hello world".to_string());
    tf.on_event(&alt_key(KeyCode::Char('b'))).await;

    assert_eq!(tf.value, "hello world");
    assert_eq!(tf.cursor_pos(), 6);
}

#[tokio::test]
async fn option_right_escape_f_moves_forward_by_word() {
    let mut tf = TextField::new("hello world".to_string());
    tf.set_cursor_pos(0);
    tf.on_event(&alt_key(KeyCode::Char('f'))).await;

    assert_eq!(tf.value, "hello world");
    assert_eq!(tf.cursor_pos(), 6);
}

#[tokio::test]
async fn terminal_state_diff_after_mutation() {
    let mut tf = TextField::new("ab".to_string());
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, tui::Theme::default(), (80, 24));

    // Initial render
    render_component_with_renderer(|ctx| tf.render(ctx), &mut renderer, 80, 24);
    assert_buffer_eq(renderer.writer(), &["ab▏"]);

    // Mutate and re-render through same Renderer (exercises diff path)
    tf.on_event(&Event::Key(key(KeyCode::Char('c')))).await;
    render_component_with_renderer(|ctx| tf.render(ctx), &mut renderer, 80, 24);
    assert_buffer_eq(renderer.writer(), &["abc▏"]);
}

#[tokio::test]
async fn ctrl_a_and_ctrl_e_move_to_hard_line_boundaries() {
    let mut tf = TextField::new("hello\nworld".to_string());
    tf.set_cursor_pos("hello\nwor".len());

    tf.on_event(&ctrl_key(KeyCode::Char('a'))).await;
    tf.on_event(&Event::Key(key(KeyCode::Char('X')))).await;
    assert_eq!(tf.value, "hello\nXworld");
    assert_eq!(tf.cursor_pos(), "hello\nX".len());

    tf.on_event(&ctrl_key(KeyCode::Char('e'))).await;
    tf.on_event(&Event::Key(key(KeyCode::Char('!')))).await;
    assert_eq!(tf.value, "hello\nXworld!");
    assert_eq!(tf.cursor_pos(), "hello\nXworld!".len());
}

#[tokio::test]
async fn vertical_movement_crosses_hard_newlines() {
    let cases = [
        ("hello\nworld", 9, 10, KeyCode::Up, 3),
        ("hello\nworld", 2, 10, KeyCode::Down, 8),
        ("ab\nlonger", 7, 10, KeyCode::Up, 2),
        ("longer\nab", 4, 10, KeyCode::Down, 9),
        ("hello world\nshort", 14, 5, KeyCode::Up, 8),
        ("hello world\nshort", 3, 5, KeyCode::Down, 9),
    ];

    for (text, cursor, width, key_code, expected) in cases {
        let mut tf = TextField::new(text.to_string());
        tf.set_cursor_pos(cursor);
        tf.set_content_width(width);
        tf.on_event(&Event::Key(key(key_code))).await;
        assert_eq!(tf.cursor_pos(), expected);
    }
}

#[tokio::test]
async fn vertical_movement_uses_soft_wrap_word_boundaries() {
    let cases = [("hello world", 11, 7, KeyCode::Up, 5), ("hello world", 3, 7, KeyCode::Down, 9)];

    for (text, cursor, width, key_code, expected) in cases {
        let mut tf = TextField::new(text.to_string());
        tf.set_cursor_pos(cursor);
        tf.set_content_width(width);
        tf.on_event(&Event::Key(key(key_code))).await;
        assert_eq!(tf.cursor_pos(), expected);
    }
}

#[test]
fn visual_line_boundaries_use_soft_wrap_word_boundaries() {
    let mut tf = TextField::new("hello world".to_string());
    tf.set_content_width(7);
    tf.set_cursor_pos(6);

    assert!(!tf.is_cursor_on_first_visual_line());
    assert!(tf.is_cursor_on_last_visual_line());
}

#[test]
fn trailing_newline_cursor_is_on_last_visual_line() {
    let mut tf = TextField::new("hello\n".to_string());
    tf.set_content_width(10);

    assert!(tf.is_cursor_on_last_visual_line());
    assert!(!tf.is_cursor_on_first_visual_line());
}

fn alt_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

fn ctrl_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

#[test]
fn unfocused_renders_without_cursor() {
    let tf = TextField::new("hello".to_string());
    let ctx = ViewContext::new((80, 24));
    let lines = tf.render_field(&ctx, false);
    let term = render_lines(&lines, 80, 24);
    assert_buffer_eq(&term, &["hello"]);
}

#[test]
fn unfocused_empty_renders_empty() {
    let tf = TextField::new(String::new());
    let ctx = ViewContext::new((80, 24));
    let lines = tf.render_field(&ctx, false);
    let term = render_lines(&lines, 80, 24);
    assert_buffer_eq(&term, &[""]);
}
