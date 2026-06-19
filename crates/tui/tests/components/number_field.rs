use super::*;
use crossterm::event::KeyCode;
use tui::NumberField;

#[test]
fn empty_renders_cursor() {
    let mut nf = NumberField::new(String::new(), false);
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["▏"]);
}

#[tokio::test]
async fn integer_input_renders() {
    let mut nf = NumberField::new(String::new(), true);
    nf.on_event(&Event::Key(key(KeyCode::Char('-')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('4')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('2')))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["-42▏"]);
}

#[tokio::test]
async fn float_input_renders() {
    let mut nf = NumberField::new(String::new(), false);
    nf.on_event(&Event::Key(key(KeyCode::Char('3')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('1')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('4')))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["3.14▏"]);
}

#[tokio::test]
async fn integer_rejects_dot() {
    let mut nf = NumberField::new(String::new(), true);
    nf.on_event(&Event::Key(key(KeyCode::Char('1')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('2')))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["12▏"]);
}

#[tokio::test]
async fn rejects_alpha() {
    let mut nf = NumberField::new(String::new(), false);
    nf.on_event(&Event::Key(key(KeyCode::Char('1')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('a')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('2')))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["12▏"]);
}

#[tokio::test]
async fn rejects_second_dot() {
    let mut nf = NumberField::new(String::new(), false);
    nf.on_event(&Event::Key(key(KeyCode::Char('1')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('2')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('3')))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["1.23▏"]);
}

#[tokio::test]
async fn backspace_renders() {
    let mut nf = NumberField::new(String::new(), true);
    nf.on_event(&Event::Key(key(KeyCode::Char('9')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Char('9')))).await;
    nf.on_event(&Event::Key(key(KeyCode::Backspace))).await;
    let term = render_component(|ctx| nf.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["9▏"]);
}

#[test]
fn unfocused_renders_without_cursor() {
    let nf = NumberField::new("42".to_string(), true);
    let ctx = ViewContext::new((80, 24));
    let lines = nf.render_field(&ctx, false);
    let term = render_lines(&lines, 80, 24);
    assert_buffer_eq(&term, &["42"]);
}
