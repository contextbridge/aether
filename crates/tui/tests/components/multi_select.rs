use super::*;
use crossterm::event::KeyCode;
use tui::MultiSelect;

#[test]
fn renders_all_unchecked() {
    let mut ms = MultiSelect::new(sample_options(), vec![false, false, false]);
    let term = render_component(|ctx| ms.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ] Alpha", "[ ] Beta", "[ ] Gamma"]);
}

#[test]
fn renders_some_checked() {
    let mut ms = MultiSelect::new(sample_options(), vec![true, false, true]);
    let term = render_component(|ctx| ms.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x] Alpha", "[ ] Beta", "[x] Gamma"]);
}

#[tokio::test]
async fn space_toggles_at_cursor() {
    let mut ms = MultiSelect::new(sample_options(), vec![false, false, false]);
    ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    let term = render_component(|ctx| ms.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x] Alpha", "[ ] Beta", "[ ] Gamma"]);
}

#[tokio::test]
async fn navigate_and_toggle_multiple() {
    let mut ms = MultiSelect::new(sample_options(), vec![false, false, false]);
    // Toggle Alpha (cursor=0)
    ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    // Move to Beta, then Gamma
    ms.on_event(&Event::Key(key(KeyCode::Down))).await;
    ms.on_event(&Event::Key(key(KeyCode::Down))).await;
    // Toggle Gamma (cursor=2)
    ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    let term = render_component(|ctx| ms.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[x] Alpha", "[ ] Beta", "[x] Gamma"]);
}

#[tokio::test]
async fn cursor_wraps_up_from_first() {
    let mut ms = MultiSelect::new(sample_options(), vec![false, false, false]);
    // Press Up from cursor=0, should wrap to cursor=2 (Gamma)
    ms.on_event(&Event::Key(key(KeyCode::Up))).await;
    assert_eq!(ms.cursor, 2);
    // Toggle at the new cursor position to verify it's on Gamma
    ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
    let term = render_component(|ctx| ms.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["[ ] Alpha", "[ ] Beta", "[x] Gamma"]);
}

#[test]
fn unfocused_none_selected_shows_none() {
    let ms = MultiSelect::new(sample_options(), vec![false, false, false]);
    let ctx = ViewContext::new((80, 24));
    let rendered = ms.render_field(&ctx, false);
    assert_eq!(rendered.len(), 1, "Unfocused should render a single inline line");
    let term = render_lines(&rendered, 80, 24);
    let lines = term.get_lines();
    assert!(lines[0].contains("(none)"), "Expected '(none)' in unfocused render, got: '{}'", lines[0]);
}

#[test]
fn unfocused_with_selections_shows_summary() {
    let ms = MultiSelect::new(sample_options(), vec![true, false, true]);
    let ctx = ViewContext::new((80, 24));
    let rendered = ms.render_field(&ctx, false);
    assert_eq!(rendered.len(), 1, "Unfocused should render a single inline line");
    let term = render_lines(&rendered, 80, 24);
    let lines = term.get_lines();
    assert!(lines[0].contains("Alpha, Gamma"), "Expected 'Alpha, Gamma' in unfocused render, got: '{}'", lines[0]);
}
