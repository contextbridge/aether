use super::*;
use crossterm::event::KeyCode;
use tui::{BorderedTextField, Color, display_width_text};

#[test]
fn empty_focused_renders_heavy_box_with_label_and_cursor() {
    let field = make_field("Name", "", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, true);
    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "▏", 20, true));
}

#[test]
fn value_renders_inside_box() {
    let field = make_field("Name", "my-agent", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, true);
    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "my-agent▏", 20, true));
}

#[test]
fn unfocused_uses_light_box_and_dimmer_border_color() {
    let field = make_field("Name", "hello", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);

    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "hello", 20, false));

    let theme = tui::Theme::default();
    let top_spans = lines[0].spans();
    assert_eq!(top_spans[0].style().fg, Some(theme.text_secondary()), "unfocused top border is text_secondary");
    let bottom_spans = lines[2].spans();
    assert_eq!(bottom_spans[0].style().fg, Some(theme.text_secondary()), "unfocused bottom border is text_secondary");
}

#[test]
fn placeholder_renders_when_value_is_empty() {
    let field = make_field("Name", "", 20).placeholder("placeholder");
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);
    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "placeholder", 20, false));
}

#[test]
fn placeholder_does_not_render_when_value_is_non_empty() {
    let field = make_field("Name", "hello", 20).placeholder("placeholder");
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);
    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "hello", 20, false));
}

#[test]
fn focused_uses_primary_border() {
    let field = make_field("Name", "hi", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, true);

    let theme = tui::Theme::default();
    assert_eq!(lines[0].spans()[0].style().fg, Some(theme.primary()));
    assert_eq!(lines[2].spans()[0].style().fg, Some(theme.primary()));
}

#[test]
fn unfocused_label_stays_readable_while_border_dims() {
    // When unfocused, border is muted but the label should still be rendered
    // with text_primary so it remains readable.
    let theme = tui::Theme::default();
    let field = make_field("Name", "", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);
    let label_span =
        lines[0].spans().iter().find(|s| s.text() == "Name").expect("top border should contain a 'Name' span");
    assert_eq!(label_span.style().fg, Some(theme.text_primary()));
}

#[test]
fn long_value_clips_at_inner_width() {
    let field = make_field("Name", "abcdefghijklmnopqrstuvwxyz", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);
    let term = render_lines(&lines, 40, 10);
    assert_buffer_eq(&term, &bordered_box("Name", "abcdefghijklmnop", 20, false));
}

#[test]
fn width_auto_grows_to_fit_label_when_unset() {
    let field = BorderedTextField::new("VeryLongLabel", String::new());
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, false);
    // Top border always has corners, so plain width >= 6 + label cols
    let top = lines[0].plain_text();
    assert!(top.starts_with("┌─ VeryLongLabel ─"), "top: {top}");
    assert!(top.ends_with('┐'), "top: {top}");
}

#[test]
fn focused_uses_heavy_glyphs_and_unfocused_uses_light_glyphs() {
    let field = make_field("Name", "", 20);
    let ctx = ViewContext::new((40, 10));

    let focused = field.render_field(&ctx, true);
    let focused_top = focused[0].plain_text();
    assert!(focused_top.contains('┏') && focused_top.contains('━') && focused_top.contains('┓'));
    assert!(focused[1].plain_text().starts_with('┃'));
    assert!(focused[2].plain_text().starts_with('┗'));

    let unfocused = field.render_field(&ctx, false);
    let unfocused_top = unfocused[0].plain_text();
    assert!(unfocused_top.contains('┌') && unfocused_top.contains('─') && unfocused_top.contains('┐'));
    assert!(unfocused[1].plain_text().starts_with('│'));
    assert!(unfocused[2].plain_text().starts_with('└'));
}

#[tokio::test]
async fn typing_forwards_to_inner_text_field() {
    let mut field = make_field("Name", "", 20);
    field.on_event(&Event::Key(key(KeyCode::Char('h')))).await;
    field.on_event(&Event::Key(key(KeyCode::Char('i')))).await;
    assert_eq!(field.value(), "hi");
}

#[test]
fn set_value_and_clear_forward_to_inner() {
    let mut field = make_field("Name", "", 20);
    field.set_value("hello".to_string());
    assert_eq!(field.value(), "hello");
    field.clear();
    assert_eq!(field.value(), "");
}

#[test]
fn border_color_is_not_affected_by_theme_bg() {
    // Sanity: border spans don't carry a background color.
    let field = make_field("Name", "", 20);
    let ctx = ViewContext::new((40, 10));
    let lines = field.render_field(&ctx, true);
    for line in &lines {
        for span in line.spans() {
            assert_ne!(span.style().bg, Some(Color::Red), "unexpected red bg");
        }
    }
}

fn bordered_box(label: &str, content: &str, width: usize, focused: bool) -> [String; 3] {
    let (top_left, top_right, bottom_left, bottom_right, horizontal, vertical) =
        if focused { ("┏", "┓", "┗", "┛", "━", "┃") } else { ("┌", "┐", "└", "┘", "─", "│") };
    let label_width = display_width_text(label);
    let dash_cols = width.saturating_sub(label_width + 5);
    let content_width = width.saturating_sub(4);
    let pad_cols = content_width.saturating_sub(display_width_text(content));

    [
        format!("{top_left}{horizontal} {label} {}{top_right}", horizontal.repeat(dash_cols)),
        format!("{vertical} {content}{} {vertical}", " ".repeat(pad_cols)),
        format!("{bottom_left}{}{bottom_right}", horizontal.repeat(width.saturating_sub(2))),
    ]
}

fn make_field(label: &str, value: &str, width: usize) -> BorderedTextField {
    let mut field = BorderedTextField::new(label.to_string(), value.to_string());
    field.set_width(width);
    field
}
