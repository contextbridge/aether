use super::*;
use crossterm::event::KeyCode;
use tui::Checkbox;

#[test]
fn unchecked_renders_bracket_space() {
    let mut cb = Checkbox::new(false);
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ]"]);
}

#[test]
fn checked_renders_bracket_x() {
    let mut cb = Checkbox::new(true);
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x]"]);
}

#[tokio::test]
async fn space_toggle_updates_render() {
    let mut cb = Checkbox::new(false);
    cb.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x]"]);
}

#[tokio::test]
async fn double_toggle_returns_to_unchecked() {
    let mut cb = Checkbox::new(false);
    cb.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    cb.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ]"]);
}

#[tokio::test]
async fn non_space_key_does_not_change_render() {
    let mut cb = Checkbox::new(false);
    cb.on_event(&Event::Key(key(KeyCode::Enter))).await;
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ]"]);
}

#[test]
fn unfocused_renders_without_highlight() {
    let cb = Checkbox::new(true);
    let ctx = ViewContext::new((80, 24));
    let lines = cb.render_field(&ctx, false);
    let term = render_lines(&lines, 80, 24);
    assert_buffer_eq(&term, &["[x]"]);
}

#[test]
fn with_label_renders_marker_and_label_inline() {
    let mut cb = Checkbox::new(true).with_label("Include AGENTS.md");
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x] Include AGENTS.md"]);
}

#[test]
fn with_label_unchecked_renders_empty_marker() {
    let mut cb = Checkbox::new(false).with_label("Enable feature");
    let term = render_component(|ctx| cb.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ] Enable feature"]);
}
