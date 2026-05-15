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

fn alt_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::ALT))
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
