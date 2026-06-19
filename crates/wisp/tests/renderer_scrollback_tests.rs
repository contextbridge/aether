use tui::Renderer;
use tui::testing::TestTerminal;
use tui::{Cursor, Frame, Line, Theme};

fn render_frame(renderer: &mut Renderer<TestTerminal>, lines: Vec<Line>, cursor: Cursor) {
    renderer.render_frame(|_ctx| Frame::new(lines).with_cursor(cursor)).unwrap();
}

#[test]
fn render_soft_wraps_before_diffing() {
    let mut renderer = create_renderer(3, 20);

    render_frame(&mut renderer, vec![Line::new("abcdef")], Cursor { row: 0, col: 5, is_visible: true });

    let lines = renderer.writer().get_lines();
    assert_eq!(lines[0], "abc");
    assert_eq!(lines[1], "def");
}

#[test]
fn push_to_scrollback_soft_wraps_long_lines() {
    let mut renderer = create_renderer(5, 20);

    render_frame(&mut renderer, vec![Line::new("abcde")], Cursor { row: 0, col: 0, is_visible: true });

    renderer.push_to_scrollback(&[Line::new("0123456789")]).unwrap();

    let transcript = renderer.writer().get_transcript_lines();
    assert!(
        transcript.iter().any(|l| l.contains("01234")),
        "expected wrapped first half in transcript: {transcript:?}"
    );
    assert!(
        transcript.iter().any(|l| l.contains("56789")),
        "expected wrapped second half in transcript: {transcript:?}"
    );
    assert!(
        !transcript.iter().any(|l| l.contains("0123456789")),
        "line should have been split by soft-wrap: {transcript:?}"
    );
}

#[test]
fn out_of_bounds_cursor_clamps_without_panicking() {
    let mut renderer = create_renderer(4, 20);

    render_frame(&mut renderer, vec![Line::new("a")], Cursor { row: 10, col: 100, is_visible: true });

    let lines = renderer.writer().get_lines();
    assert_eq!(lines[0], "a");
}

#[test]
fn render_flushes_overflow_to_scrollback() {
    let mut renderer = create_renderer(20, 3);

    render_frame(
        &mut renderer,
        vec![Line::new("L1"), Line::new("L2"), Line::new("L3"), Line::new("L4"), Line::new("L5")],
        Cursor { row: 4, col: 0, is_visible: true },
    );

    let visible = renderer.writer().get_lines();
    assert_eq!(visible[0], "L3");
    assert_eq!(visible[1], "L4");
    assert_eq!(visible[2], "L5");

    let transcript = renderer.writer().get_transcript_lines();
    assert!(transcript.iter().any(|l| l == "L1"), "L1 should be in scrollback: {transcript:?}");
    assert!(transcript.iter().any(|l| l == "L2"), "L2 should be in scrollback: {transcript:?}");
}

#[test]
fn render_progressively_flushes_overflow() {
    let mut renderer = create_renderer(20, 3);

    render_frame(
        &mut renderer,
        vec![Line::new("L1"), Line::new("L2"), Line::new("L3"), Line::new("L4")],
        Cursor { row: 3, col: 0, is_visible: true },
    );

    let transcript_after_first = renderer.writer().get_transcript_lines();
    assert!(
        transcript_after_first.iter().any(|l| l == "L1"),
        "L1 should be flushed after first render: {transcript_after_first:?}"
    );

    render_frame(
        &mut renderer,
        vec![Line::new("L1"), Line::new("L2"), Line::new("L3"), Line::new("L4"), Line::new("L5"), Line::new("L6")],
        Cursor { row: 5, col: 0, is_visible: true },
    );

    let transcript_after_second = renderer.writer().get_transcript_lines();
    assert!(
        transcript_after_second.iter().any(|l| l == "L2"),
        "L2 should be in transcript after second render: {transcript_after_second:?}"
    );
    assert!(
        transcript_after_second.iter().any(|l| l == "L3"),
        "L3 should be in transcript after second render: {transcript_after_second:?}"
    );

    let visible = renderer.writer().get_lines();
    assert_eq!(visible[0], "L4");
    assert_eq!(visible[1], "L5");
    assert_eq!(visible[2], "L6");
}

#[test]
fn push_to_scrollback_resets_flushed_count() {
    let mut renderer = create_renderer(20, 3);

    render_frame(
        &mut renderer,
        vec![Line::new("L1"), Line::new("L2"), Line::new("L3"), Line::new("L4"), Line::new("L5")],
        Cursor { row: 4, col: 0, is_visible: true },
    );

    renderer.push_to_scrollback(&[Line::new("committed")]).unwrap();

    render_frame(
        &mut renderer,
        vec![Line::new("A1"), Line::new("A2"), Line::new("A3"), Line::new("A4"), Line::new("A5")],
        Cursor { row: 4, col: 0, is_visible: true },
    );

    let transcript = renderer.writer().get_transcript_lines();
    assert!(
        transcript.iter().any(|l| l == "A1"),
        "A1 should be in scrollback (proves counter was reset): {transcript:?}"
    );
    assert!(
        transcript.iter().any(|l| l == "A2"),
        "A2 should be in scrollback (proves counter was reset): {transcript:?}"
    );
}

#[test]
fn reflow_above_overflow_boundary_does_not_duplicate_a_line_in_the_transcript() {
    let mut renderer = create_renderer(10, 3);

    render_frame(
        &mut renderer,
        vec![
            Line::new("hello worl"),
            Line::new("L2"),
            Line::new("L3"),
            Line::new("L4"),
            Line::new("L5"),
            Line::new("L6"),
        ],
        Cursor { row: 5, col: 0, is_visible: true },
    );

    render_frame(
        &mut renderer,
        vec![Line::new("hello world"), Line::new("L2"), Line::new("L3"), Line::new("L4"), Line::new("L5")],
        Cursor { row: 4, col: 0, is_visible: true },
    );

    let transcript = renderer.writer().get_transcript_lines();
    let visible = renderer.writer().get_lines();
    assert_eq!(visible, vec!["L3", "L4", "L5"], "viewport should show the last three lines");
    for line in ["L2", "L3", "L4", "L5"] {
        let count = transcript.iter().filter(|l| *l == line).count();
        assert_eq!(count, 1, "{line} should appear exactly once in transcript: {transcript:?}");
    }
}

fn create_renderer(cols: u16, rows: u16) -> Renderer<TestTerminal> {
    let terminal = TestTerminal::new(cols, rows);
    Renderer::new(terminal, Theme::default(), (cols, rows))
}
