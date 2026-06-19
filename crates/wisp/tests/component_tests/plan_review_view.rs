use tui::testing::{key, render_component};
use tui::{Component, Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind, ViewContext};
use wisp::components::app::{PlanReviewAction, PlanReviewInput, PlanReviewMode};
use wisp::components::plan_review::PlanDocument;

const OUTLINE_HINT_LINE: &str = "j/k move  g/G top/bottom  enter jump  h/l focus  u undo  Esc cancel";
const PLAN_HINT_LINE: &str = "j/k move  n/p heading  h outline  c comment  u undo  a approve  r changes  Esc cancel";

fn make_mode(markdown: &str) -> PlanReviewMode {
    PlanReviewMode::new(PlanReviewInput {
        title: "Review /tmp/test-plan.md".to_string(),
        document: PlanDocument::parse("/tmp/test-plan.md", markdown),
    })
}

fn render_mode(mode: &mut PlanReviewMode, width: u16, height: u16) {
    let ctx = ViewContext::new((width, height));
    let _ = mode.render(&ctx);
}

fn mouse_scroll(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE })
}

async fn send_keys_with_render(mode: &mut PlanReviewMode, codes: &[KeyCode], width: u16, height: u16) {
    for &code in codes {
        render_mode(mode, width, height);
        mode.on_event(&Event::Key(key(code))).await;
    }
}

#[tokio::test]
async fn plan_review_sidebar_and_separator_match_git_diff_chrome() {
    let mut mode = make_mode("# Intro\n\nbody\n\n## Details\n\nmore");
    let term = render_component(|ctx| mode.render(ctx), 100, 12);
    let ctx = ViewContext::new((100, 12));
    let lines = term.get_lines();
    let outline_header = lines.iter().position(|line| line.contains("Outline")).expect("outline header should render");
    let separator_col = lines[outline_header].find('│').expect("split separator should render as a vertical rule");

    assert_eq!(term.get_style_at(outline_header, separator_col).bg, None, "split separator should not set bg");
    assert_eq!(term.get_style_at(outline_header, separator_col).fg, Some(ctx.theme.muted()));
    assert_eq!(term.get_style_at(outline_header, 1).bg, None, "outline header should not set explicit bg");

    let body_row = lines.iter().position(|line| line.contains("Details")).expect("outline section should render");
    assert_eq!(term.get_style_at(body_row, 1).bg, None, "unselected outline rows should not set explicit bg");
}

#[tokio::test]
async fn focused_panel_title_and_footer_follow_focus() {
    let mut mode = make_mode("# Intro\n\nbody\n\n## Details\n\nmore");
    let ctx = ViewContext::new((100, 12));

    let term = render_component(|ctx| mode.render(ctx), 100, 12);
    let lines = term.get_lines();
    let outline_header = lines.iter().position(|line| line.contains("Outline")).expect("outline header should render");
    assert_eq!(lines.last().map(String::as_str), Some(PLAN_HINT_LINE));
    assert_eq!(term.get_style_at(outline_header, 1).fg, Some(ctx.theme.text_primary()));

    send_keys_with_render(&mut mode, &[KeyCode::Char('h')], 100, 12).await;

    let term = render_component(|ctx| mode.render(ctx), 100, 12);
    let lines = term.get_lines();
    let outline_header = lines.iter().position(|line| line.contains("Outline")).expect("outline header should render");
    assert_eq!(lines.last().map(String::as_str), Some(OUTLINE_HINT_LINE));
    assert_eq!(term.get_style_at(outline_header, 1).fg, Some(ctx.theme.accent()));
}
#[tokio::test]
async fn cursor_navigation_moves_between_source_lines() {
    let mut mode = make_mode("# One\n- line_one\n- line_two\n- line_three");
    render_mode(&mut mode, 80, 24);

    assert_eq!(mode.current_source_line_no(), 1);

    send_keys_with_render(&mut mode, &[KeyCode::Char('j')], 80, 24).await;
    assert_eq!(mode.current_source_line_no(), 2);

    send_keys_with_render(&mut mode, &[KeyCode::Char('G')], 80, 24).await;
    assert_eq!(mode.current_source_line_no(), 4);

    send_keys_with_render(&mut mode, &[KeyCode::Char('g')], 80, 24).await;
    assert_eq!(mode.current_source_line_no(), 1);
}

#[tokio::test]
async fn mouse_scroll_moves_plan_by_one_source_line() {
    let mut mode = make_mode("# One\n- line_one\n- line_two\n- line_three");
    render_mode(&mut mode, 80, 24);

    mode.on_event(&mouse_scroll(MouseEventKind::ScrollDown)).await;
    assert_eq!(mode.current_source_line_no(), 2);

    mode.on_event(&mouse_scroll(MouseEventKind::ScrollDown)).await;
    assert_eq!(mode.current_source_line_no(), 3);

    mode.on_event(&mouse_scroll(MouseEventKind::ScrollUp)).await;
    assert_eq!(mode.current_source_line_no(), 2);
}

#[tokio::test]
async fn outline_selection_owns_navigation_until_enter_jumps_document() {
    let mut mode = make_mode("# One\n\nbody first line\nbody second line\n\n## Two\n\nmore");
    render_mode(&mut mode, 80, 24);

    send_keys_with_render(&mut mode, &[KeyCode::Char('h'), KeyCode::Char('j')], 80, 24).await;
    assert_eq!(mode.current_source_line_no(), 1, "moving the outline should not move the document cursor");

    send_keys_with_render(&mut mode, &[KeyCode::Enter], 80, 24).await;
    assert_eq!(
        mode.current_source_line_no(),
        6,
        "enter should jump the document cursor to the selected outline section"
    );
}

#[tokio::test]
async fn plan_review_uses_shared_inline_styling() {
    let mut mode = make_mode("# Intro\nThis has **bold**, *italic*, `code`, and [link](https://example.com).");
    let theme_ctx = ViewContext::new((120, 12));
    let terminal = render_component(|ctx| mode.render(ctx), 120, 12);
    let lines = terminal.get_lines();
    let row = lines
        .iter()
        .position(|line| line.contains("This has bold, italic, code, and link."))
        .expect("styled line should render without markdown markers");

    assert!(terminal.style_of_text(row, "bold").unwrap().bold);
    assert!(terminal.style_of_text(row, "italic").unwrap().italic);
    assert_eq!(terminal.style_of_text(row, "code").unwrap().fg, Some(theme_ctx.theme.code_fg()));
    let link_style = terminal.style_of_text(row, "link").unwrap();
    assert!(link_style.underline);
    assert_eq!(link_style.fg, Some(theme_ctx.theme.link()));
}

#[tokio::test]
async fn plan_body_soft_wraps_long_markdown_lines() {
    let long_line = "x".repeat(140);
    let markdown = format!("# Intro\n\n{long_line}\n\nshort_tail");
    let mut mode = make_mode(&markdown);

    send_keys_with_render(&mut mode, &[KeyCode::Char('j')], 50, 12).await;

    let terminal = render_component(|ctx| mode.render(ctx), 50, 12);
    let lines = terminal.get_lines();
    let wrapped_rows = lines.iter().filter(|line| line.contains("xxxx")).count();

    assert!(wrapped_rows > 1, "expected long markdown line to soft wrap, got lines: {lines:?}");
}

#[tokio::test]
async fn plan_review_renders_line_number_for_every_source_line() {
    let markdown = "# Intro\n\nfirst paragraph line\nsecond paragraph line\n```rust\nlet x = 1;\n```\n| A | B |\n|---|---|\n| 1 | 2 |";
    let mut mode = make_mode(markdown);

    let terminal = render_component(|ctx| mode.render(ctx), 120, 24);
    let text = terminal.get_lines().join("\n");

    for line_no in 1..=markdown.split('\n').count() {
        let gutter = format!("{line_no:>2} │");
        assert!(text.contains(&gutter), "missing gutter {gutter:?} in:\n{text}");
    }
}

#[tokio::test]
async fn inline_comment_renders_below_wrapped_source_line() {
    let long_line = "x".repeat(140);
    let markdown = format!("# Intro\n\n{long_line}\n\nshort_tail");
    let mut mode = make_mode(&markdown);

    send_keys_with_render(
        &mut mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('n'),
            KeyCode::Char('o'),
            KeyCode::Char('t'),
            KeyCode::Char('e'),
            KeyCode::Enter,
        ],
        80,
        20,
    )
    .await;

    let terminal = render_component(|ctx| mode.render(ctx), 80, 20);
    let lines = terminal.get_lines();

    let first_long_row = lines.iter().position(|line| line.contains("xxxx")).expect("long line should render");
    let last_long_row = lines.iter().rposition(|line| line.contains("xxxx")).expect("long line should render");
    let comment_row = lines.iter().position(|line| line.contains("note")).expect("comment should render");
    let tail_row = lines.iter().position(|line| line.contains("short_tail")).expect("tail line should render");

    assert!(last_long_row > first_long_row, "expected long anchor line to wrap, got lines: {lines:?}");
    assert!(comment_row > last_long_row, "comment should render after wrapped anchor block");
    assert!(tail_row > comment_row, "following block should remain below the comment");
}

#[tokio::test]
async fn inline_comment_and_draft_render_below_their_source_lines() {
    let mut mode = make_mode("# Intro\n\nline_one\n\nline_two\n\nline_three");

    send_keys_with_render(
        &mut mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('f'),
            KeyCode::Char('i'),
            KeyCode::Char('r'),
            KeyCode::Char('s'),
            KeyCode::Char('t'),
            KeyCode::Enter,
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('d'),
            KeyCode::Char('r'),
            KeyCode::Char('a'),
            KeyCode::Char('f'),
            KeyCode::Char('t'),
        ],
        100,
        22,
    )
    .await;

    let terminal = render_component(|ctx| mode.render(ctx), 100, 22);
    let lines = terminal.get_lines();

    let line_one_row = lines.iter().position(|line| line.contains("line_one")).expect("line_one should render");
    let submitted_row = lines.iter().position(|line| line.contains("first")).expect("submitted comment should render");
    let line_two_row = lines.iter().position(|line| line.contains("line_two")).expect("line_two should render");
    let line_three_row = lines.iter().position(|line| line.contains("line_three")).expect("line_three should render");
    let draft_row = lines.iter().position(|line| line.contains("draft")).expect("draft comment should render");

    assert!(submitted_row > line_one_row);
    assert!(line_two_row > submitted_row);
    assert!(draft_row > line_two_row);
    assert!(line_three_row > draft_row);
}

#[tokio::test]
async fn submitted_comment_on_last_block_stays_visible_at_bottom_of_viewport() {
    let mut mode = make_mode("# Intro\n\nline_one\n\nline_two\n\nline_three");

    send_keys_with_render(
        &mut mode,
        &[KeyCode::Char('G'), KeyCode::Char('c'), KeyCode::Char('h'), KeyCode::Char('i'), KeyCode::Enter],
        100,
        7,
    )
    .await;

    let terminal = render_component(|ctx| mode.render(ctx), 100, 7);
    let lines = terminal.get_lines();

    assert!(lines.iter().any(|line| line.contains("line_three")), "cursor line should remain visible");
    assert!(lines.iter().any(|line| line.contains("hi")), "comment text should be visible");
    assert!(lines.iter().any(|line| line.contains('└')), "comment bottom border should be visible");
}

#[tokio::test]
async fn wrapped_plan_source_line_is_one_navigation_stop() {
    let long_line = "x".repeat(140);
    let markdown = format!("# Intro\n\n{long_line}\n\nshort_tail");
    let mut mode = make_mode(&markdown);

    send_keys_with_render(&mut mode, &[KeyCode::Char('j'), KeyCode::Char('j')], 50, 12).await;
    assert_eq!(mode.current_source_line_no(), 3, "second j should land on the wrapped source line");

    send_keys_with_render(&mut mode, &[KeyCode::Char('j')], 50, 12).await;
    assert_eq!(
        mode.current_source_line_no(),
        4,
        "third j should move to the next source line, not a wrapped continuation row"
    );
}

#[tokio::test]
async fn wrapped_plan_lines_highlight_only_the_active_visual_row() {
    let long_line = "x".repeat(140);
    let markdown = format!("# Intro\n\n{long_line}\n\nshort_tail");
    let mut mode = make_mode(&markdown);

    send_keys_with_render(&mut mode, &[KeyCode::Char('j'), KeyCode::Char('j')], 50, 12).await;

    let ctx = ViewContext::new((50, 12));
    let terminal = render_component(|render_ctx| mode.render(render_ctx), 50, 12);
    let lines = terminal.get_lines();
    let highlight_bg = ctx.theme.highlight_bg();
    let highlighted_wrapped_rows = lines
        .iter()
        .enumerate()
        .filter(|(row, line)| {
            line.contains("xxxx")
                && line.find('x').is_some_and(|col| terminal.get_style_at(*row, col).bg == Some(highlight_bg))
        })
        .count();

    assert_eq!(highlighted_wrapped_rows, 1, "expected exactly one wrapped visual row to be highlighted");
}

#[tokio::test]
async fn request_changes_feedback_keys_to_exact_source_line() {
    let mut mode = make_mode("# Intro\n\nfirst line\nsecond line\n\n## Details\n\nmore");

    send_keys_with_render(
        &mut mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('f'),
            KeyCode::Char('i'),
            KeyCode::Char('x'),
            KeyCode::Enter,
        ],
        80,
        24,
    )
    .await;

    let deny_action = mode
        .on_event(&Event::Key(key(KeyCode::Char('r'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("deny should emit an action");
    let PlanReviewAction::RequestChanges { feedback } = deny_action else {
        panic!("expected request changes action");
    };

    assert!(feedback.contains("Line 4"), "feedback should use the exact source line: {feedback}");
    assert!(feedback.contains("`second line`"), "feedback should quote the original source text: {feedback}");
}

#[tokio::test]
async fn comment_on_blank_line_reports_line_without_snippet() {
    let mut mode = make_mode("# Intro\n\nbody");

    send_keys_with_render(
        &mut mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('b'),
            KeyCode::Char('l'),
            KeyCode::Char('a'),
            KeyCode::Char('n'),
            KeyCode::Char('k'),
            KeyCode::Enter,
        ],
        80,
        24,
    )
    .await;

    let action = mode
        .on_event(&Event::Key(key(KeyCode::Char('r'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("request changes should emit an action");
    let PlanReviewAction::RequestChanges { feedback } = action else {
        panic!("expected request changes action");
    };

    assert!(feedback.contains("Line 2"), "feedback should use the blank source line: {feedback}");
    assert!(!feedback.contains("``"), "blank line should not produce a source snippet: {feedback}");
    assert!(feedback.contains("blank"));
}

#[tokio::test]
async fn comment_inside_fenced_code_uses_code_source_line() {
    let mut mode = make_mode("# Intro\n\n```rust\nlet x = 1;\n```");

    send_keys_with_render(
        &mut mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('c'),
            KeyCode::Char('o'),
            KeyCode::Char('d'),
            KeyCode::Char('e'),
            KeyCode::Enter,
        ],
        80,
        24,
    )
    .await;

    let action = mode
        .on_event(&Event::Key(key(KeyCode::Char('r'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("request changes should emit an action");
    let PlanReviewAction::RequestChanges { feedback } = action else {
        panic!("expected request changes action");
    };

    assert!(feedback.contains("Line 4"), "feedback should use the code source line: {feedback}");
    assert!(feedback.contains("`let x = 1;`"), "feedback should quote the code source line: {feedback}");
}

#[tokio::test]
async fn approve_request_changes_and_cancel_emit_expected_actions() {
    let mut approve_mode = make_mode("# Intro\nline_one");
    let approve_action = approve_mode
        .on_event(&Event::Key(key(KeyCode::Char('a'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("approve should emit an action");
    assert!(matches!(approve_action, PlanReviewAction::Approve));

    let mut deny_mode = make_mode("# Intro\n\nline_one");
    send_keys_with_render(
        &mut deny_mode,
        &[
            KeyCode::Char('j'),
            KeyCode::Char('c'),
            KeyCode::Char('n'),
            KeyCode::Char('e'),
            KeyCode::Char('e'),
            KeyCode::Char('d'),
            KeyCode::Enter,
        ],
        80,
        24,
    )
    .await;
    let deny_action = deny_mode
        .on_event(&Event::Key(key(KeyCode::Char('r'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("deny should emit an action");
    let PlanReviewAction::RequestChanges { feedback } = deny_action else {
        panic!("expected request changes action");
    };
    assert!(feedback.contains("need"));

    let mut deny_without_comments_mode = make_mode("# Intro\nline_one");
    let fallback_action = deny_without_comments_mode
        .on_event(&Event::Key(key(KeyCode::Char('r'))))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("deny should emit an action");
    let PlanReviewAction::RequestChanges { feedback } = fallback_action else {
        panic!("expected request changes action");
    };
    assert!(feedback.contains("no inline comments"));

    let mut cancel_mode = make_mode("# Intro\nline_one");
    let cancel_action = cancel_mode
        .on_event(&Event::Key(key(KeyCode::Esc)))
        .await
        .and_then(|mut msgs| msgs.pop())
        .expect("cancel should emit an action");
    assert!(matches!(cancel_action, PlanReviewAction::Cancel));
}
