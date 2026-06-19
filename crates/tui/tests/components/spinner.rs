use super::*;
use tui::Renderer;
use tui::{BRAILLE_FRAMES, Spinner};

#[test]
fn invisible_renders_empty() {
    let mut spinner = Spinner::default();
    spinner.visible = false;
    let ctx = ViewContext::new((80, 24));
    let frame = spinner.render(&ctx);
    assert!(frame.lines().is_empty());
    // Also verify it produces an empty buffer
    let term = render_lines(frame.lines(), 80, 24);
    assert_buffer_eq(&term, &[""]);
}

#[test]
fn visible_tick_0_renders_first_frame() {
    let mut spinner = Spinner::default();
    spinner.visible = true;
    spinner.set_tick(0);
    let term = render_component(|ctx| spinner.render(ctx), 80, 24);
    let lines = term.get_lines();
    let expected = BRAILLE_FRAMES[0].to_string();
    assert!(lines[0].contains(&expected), "Expected '{}' in buffer, got: '{}'", expected, lines[0]);
}

#[test]
fn tick_1_renders_second_frame() {
    let mut spinner = Spinner::default();
    spinner.visible = true;
    spinner.set_tick(1);
    let term = render_component(|ctx| spinner.render(ctx), 80, 24);
    let lines = term.get_lines();
    let expected = BRAILLE_FRAMES[1].to_string();
    assert!(lines[0].contains(&expected), "Expected '{}' in buffer, got: '{}'", expected, lines[0]);
}

#[test]
fn rerender_updates_frame_in_place() {
    let mut spinner = Spinner::default();
    spinner.visible = true;
    spinner.set_tick(0);

    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, tui::Theme::default(), (80, 24));

    // Initial render
    render_component_with_renderer(|ctx| spinner.render(ctx), &mut renderer, 80, 24);
    let first_frame = BRAILLE_FRAMES[0].to_string();
    assert!(renderer.writer().get_lines()[0].contains(&first_frame));

    // Advance tick and re-render through the same Renderer
    spinner.set_tick(1);
    render_component_with_renderer(|ctx| spinner.render(ctx), &mut renderer, 80, 24);
    let second_frame = BRAILLE_FRAMES[1].to_string();
    assert!(
        renderer.writer().get_lines()[0].contains(&second_frame),
        "Expected '{}' after re-render, got: '{}'",
        second_frame,
        renderer.writer().get_lines()[0]
    );
}
