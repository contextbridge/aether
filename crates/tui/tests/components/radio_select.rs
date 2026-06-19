use super::*;
use crossterm::event::KeyCode;
use tui::RadioSelect;

#[test]
fn renders_all_options_first_selected() {
    let mut rs = RadioSelect::new(sample_options(), 0);
    let term = render_component(|ctx| rs.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["● Alpha", "○ Beta", "○ Gamma"]);
}

#[test]
fn renders_second_selected() {
    let mut rs = RadioSelect::new(sample_options(), 1);
    let term = render_component(|ctx| rs.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["○ Alpha", "● Beta", "○ Gamma"]);
}

#[tokio::test]
async fn down_arrow_changes_selection() {
    let mut rs = RadioSelect::new(sample_options(), 0);
    rs.on_event(&Event::Key(key(KeyCode::Down))).await;
    let term = render_component(|ctx| rs.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["○ Alpha", "● Beta", "○ Gamma"]);
}

#[tokio::test]
async fn up_from_first_wraps_to_last() {
    let mut rs = RadioSelect::new(sample_options(), 0);
    rs.on_event(&Event::Key(key(KeyCode::Up))).await;
    let term = render_component(|ctx| rs.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["○ Alpha", "○ Beta", "● Gamma"]);
}

#[tokio::test]
async fn down_from_last_wraps_to_first() {
    let mut rs = RadioSelect::new(sample_options(), 2);
    rs.on_event(&Event::Key(key(KeyCode::Down))).await;
    let term = render_component(|ctx| rs.render(ctx), 80, 24);
    assert_buffer_eq(&term, &["● Alpha", "○ Beta", "○ Gamma"]);
}

#[test]
fn unfocused_renders_selected_title_inline() {
    let rs = RadioSelect::new(sample_options(), 1);
    let ctx = ViewContext::new((80, 24));
    let lines = rs.render_field(&ctx, false);
    assert_eq!(lines.len(), 1, "Unfocused should render a single inline line");
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("Beta"), "Expected 'Beta' in unfocused render, got: '{}'", output[0]);
}
