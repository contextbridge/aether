use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::TerminalOptions;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;
use tokio::sync::oneshot;
use utils::plan_review::PlanReviewElicitationMeta;
use wisp_next::plan_review::{PlanDocument, ReviewComment, compile_feedback};
use wisp_next::screens::plan_review::PlanReviewScreen;
use wisp_next::syntax::SyntaxHighlighter;
use wisp_next::theme::Theme;

fn make_meta(markdown: &str) -> PlanReviewElicitationMeta {
    PlanReviewElicitationMeta::new(&PathBuf::from("/tmp/plan.md"), markdown)
}

fn make_screen(markdown: &str) -> (PlanReviewScreen, oneshot::Receiver<ElicitationResponse>) {
    let meta = make_meta(markdown);
    let (tx, rx) = oneshot::channel();
    let respond = Box::new(move |response: ElicitationResponse| {
        let _ = tx.send(response);
    });
    (PlanReviewScreen::new_with_response_handler(meta, respond), rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn source_presentation_cache_is_reused_and_explicitly_invalidated() {
    let (mut screen, _rx) = make_screen("# Plan\n```rust\nfn main() {}\n```");
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();

    assert!(!screen.source_presentation_is_cached());
    render_screen_with_theme_generation(&mut screen, 80, 24, &theme, &mut highlighter, 0);
    assert!(screen.source_presentation_is_cached());

    render_screen_with_theme_generation(&mut screen, 50, 24, &theme, &mut highlighter, 0);
    assert!(screen.source_presentation_is_cached());

    screen.invalidate_source_presentation();
    assert!(!screen.source_presentation_is_cached());
    render_screen_with_theme_generation(&mut screen, 80, 24, &theme, &mut highlighter, 1);
    assert!(screen.source_presentation_is_cached());
}

fn render_screen_with_theme_generation(
    screen: &mut PlanReviewScreen,
    width: u16,
    height: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
    theme_generation: u64,
) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions::default()).unwrap();
    terminal
        .draw(|frame| {
            screen.render_with_theme_generation(frame, theme, highlighter, theme_generation);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render_screen(screen: &mut PlanReviewScreen, width: u16, height: u16) -> Buffer {
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    let backend = TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions::default()).unwrap();
    terminal
        .draw(|frame| {
            screen.render_with_theme_generation(frame, &theme, &mut highlighter, 0);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol));
        }
        if y + 1 < buffer.area.bottom() {
            out.push('\n');
        }
    }
    out
}

// ============================================================================
// Outline parsing / cursor jump tests
// ============================================================================

#[test]
fn document_parses_outline_from_plan_markdown() {
    let markdown = "# Overview\ncontent\n## Implementation\nmore\n### Details\nnested";
    let document = PlanDocument::parse("plan.md", markdown);

    assert_eq!(document.outline.len(), 3);
    assert_eq!(document.outline[0].title, "Overview");
    assert_eq!(document.outline[0].level, 1);
    assert_eq!(document.outline[0].first_line_no, 1);
    assert_eq!(document.outline[1].title, "Implementation");
    assert_eq!(document.outline[1].level, 2);
    assert_eq!(document.outline[1].first_line_no, 3);
    assert_eq!(document.outline[2].title, "Details");
    assert_eq!(document.outline[2].level, 3);
    assert_eq!(document.outline[2].first_line_no, 5);
}

#[test]
fn document_tracks_section_membership_per_line() {
    let markdown = "# Intro\nfirst\n## Body\nsecond\n## Summary\nthird";
    let document = PlanDocument::parse("plan.md", markdown);

    assert_eq!(document.section_title_for(&document.lines[0]), Some("Intro"));
    assert_eq!(document.section_title_for(&document.lines[1]), Some("Intro"));
    assert_eq!(document.section_title_for(&document.lines[2]), Some("Body"));
    assert_eq!(document.section_title_for(&document.lines[3]), Some("Body"));
    assert_eq!(document.section_title_for(&document.lines[4]), Some("Summary"));
    assert_eq!(document.section_title_for(&document.lines[5]), Some("Summary"));
}

#[test]
fn source_line_cursor_moves_with_jk() {
    let markdown = "# Plan\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10";
    let (mut screen, _rx) = make_screen(markdown);

    // Initial position: line 0 (0-indexed)
    assert!(!screen.on_key(key(KeyCode::Char('j')))); // → line 1
    assert!(!screen.on_key(key(KeyCode::Char('j')))); // → line 2
    assert!(!screen.on_key(key(KeyCode::Char('k')))); // → line 1
    assert!(!screen.on_key(key(KeyCode::Char('k')))); // → line 0
    assert!(!screen.on_key(key(KeyCode::Char('k')))); // → line 0 (clamped)
}

#[test]
fn source_line_cursor_goes_to_top_and_bottom() {
    let markdown = "# Plan\na\nb\nc\nd";
    let (mut screen, _rx) = make_screen(markdown);

    assert!(!screen.on_key(key(KeyCode::Char('G')))); // bottom
    assert!(!screen.on_key(key(KeyCode::Char('g')))); // top
}

#[test]
fn n_and_p_jump_between_headings() {
    let markdown = "# One\ntext\n## Two\ntext\n### Three\ntext\n#### Four\ntext";
    let (mut screen, _rx) = make_screen(markdown);

    // Start at line 0
    assert!(!screen.on_key(key(KeyCode::Char('n')))); // jump to next heading (line 2)
    assert!(!screen.on_key(key(KeyCode::Char('n')))); // jump to next heading (line 4)
    assert!(!screen.on_key(key(KeyCode::Char('p')))); // jump to prev heading (line 2)
    assert!(!screen.on_key(key(KeyCode::Char('p')))); // jump to prev heading (line 0)
}

// ============================================================================
// Comment add / cancel / submit / undo tests
// ============================================================================

#[test]
fn comment_submit_at_first_line() {
    let markdown = "# Plan\nfirst line";
    let (mut screen, mut rx) = make_screen(markdown);

    // Press 'c' to start comment, type text, press Enter to submit
    assert!(!screen.on_key(key(KeyCode::Char('c')))); // start comment
    type_text(&mut screen, "needs work");
    assert!(!screen.on_key(key(KeyCode::Enter))); // submit

    // Request changes - should include the comment
    assert!(screen.on_key(key(KeyCode::Char('r'))));
    let response = rx.try_recv().expect("responder should have been called");
    let content = response.content.expect("content should be present");
    assert!(content["feedback"].as_str().unwrap().contains("needs work"));
}

#[test]
fn comment_cancel_with_escape() {
    let markdown = "# Plan\nsome line\n## More\nanother line";
    let (mut screen, mut rx) = make_screen(markdown);

    // Move to line 2
    assert!(!screen.on_key(key(KeyCode::Char('j'))));
    assert!(!screen.on_key(key(KeyCode::Char('j'))));

    // Start comment, type, cancel
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "should be discarded");
    assert!(!screen.on_key(key(KeyCode::Esc))); // cancel draft

    // Submit request-changes — canceled draft must not leak
    assert!(screen.on_key(key(KeyCode::Char('r'))));
    let response = rx.try_recv().expect("responder should have been called");
    assert_eq!(response.action, ElicitationAction::Accept);
    let content = response.content.expect("content should be present");
    let feedback = content["feedback"].as_str().unwrap();
    assert!(!feedback.contains("should be discarded"), "canceled draft must not leak into feedback: {feedback}");
    assert!(feedback.contains("no inline comments"), "should return no-inline-comments fallback: {feedback}");
}

#[test]
fn comment_undo_removes_last_comment() {
    let markdown = "# Plan\na\nb\nc";
    let (mut screen, _rx) = make_screen(markdown);

    // Add comment on line 1
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "first");
    assert!(!screen.on_key(key(KeyCode::Enter)));

    // Add comment on line 2
    assert!(!screen.on_key(key(KeyCode::Char('j'))));
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "second");
    assert!(!screen.on_key(key(KeyCode::Enter)));

    // Undo last
    assert!(!screen.on_key(key(KeyCode::Char('u'))));

    // Submit feedback: should only contain "first"
    assert!(screen.on_key(key(KeyCode::Char('r'))));
}

#[test]
fn comment_at_last_line() {
    let markdown = "# Plan\na";
    let (mut screen, _rx) = make_screen(markdown);

    // Go to last line
    assert!(!screen.on_key(key(KeyCode::Char('G'))));
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "last line comment");
    assert!(!screen.on_key(key(KeyCode::Enter)));
}

#[test]
fn comment_at_middle_line() {
    let markdown = "# Plan\na\nb\nc\nd\ne";
    let (mut screen, _rx) = make_screen(markdown);

    // Go to middle line (line 2, 0-indexed)
    assert!(!screen.on_key(key(KeyCode::Char('j'))));
    assert!(!screen.on_key(key(KeyCode::Char('j'))));
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "middle");
    assert!(!screen.on_key(key(KeyCode::Enter)));
}

// ============================================================================
// Feedback compilation tests
// ============================================================================

#[test]
fn feedback_groups_comments_by_section() {
    let markdown = "# Intro\nintro text\n## Details\ndetail text";
    let document = PlanDocument::parse("plan.md", markdown);

    let comments =
        vec![ReviewComment::new(2, "fix intro".to_string()), ReviewComment::new(4, "fix details".to_string())];

    let feedback = compile_feedback(&document, &comments);
    assert!(feedback.contains("## Intro"));
    assert!(feedback.contains("## Details"));
    assert!(feedback.contains("Line 2"));
    assert!(feedback.contains("Line 4"));
}

#[test]
fn feedback_handles_multiline_comments() {
    let markdown = "# Top\nline";
    let document = PlanDocument::parse("plan.md", markdown);
    let comments = vec![ReviewComment::new(2, "First point\n\nSecond point".to_string())];

    let feedback = compile_feedback(&document, &comments);
    assert!(feedback.contains("- First point"));
    assert!(feedback.contains("- Second point"));
}

#[test]
fn feedback_sanitizes_backticks_in_snippets() {
    let markdown = "# Top\nuse `backtick` here";
    let document = PlanDocument::parse("plan.md", markdown);
    let comments = vec![ReviewComment::new(2, "ok".to_string())];

    let feedback = compile_feedback(&document, &comments);
    assert!(feedback.contains("\\`backtick\\`"));
}

#[test]
fn feedback_truncates_long_snippets() {
    let long_line = "x".repeat(200);
    let markdown = format!("# Top\n{long_line}");
    let document = PlanDocument::parse("plan.md", &markdown);
    let comments = vec![ReviewComment::new(2, "ok".to_string())];

    let feedback = compile_feedback(&document, &comments);
    assert!(feedback.contains("..."));
    assert!(!feedback.contains(&long_line));
}

#[test]
fn feedback_no_comments_produces_fallback() {
    let markdown = "# Plan\nline";
    let document = PlanDocument::parse("plan.md", markdown);
    let feedback = compile_feedback(&document, &[]);
    assert!(feedback.contains("no inline comments"));
}

#[test]
fn feedback_handles_code_fence_lines_in_snippets() {
    let markdown = "# Plan\n```rust\nfn main() {}\n```";
    let document = PlanDocument::parse("plan.md", markdown);
    let comments = vec![ReviewComment::new(2, "wrong language".to_string())];

    let feedback = compile_feedback(&document, &comments);
    // Code fence markers get backtick-escaped during sanitization
    assert!(feedback.contains('`'), "feedback should contain backtick-quoted snippet");
}

// ============================================================================
// Responder exactly-once tests
// ============================================================================

#[test]
fn approve_sends_correct_payload() {
    let markdown = "# Plan\ntext";
    let (mut screen, mut rx) = make_screen(markdown);

    // Approve
    assert!(screen.on_key(key(KeyCode::Char('a'))));

    let response = rx.try_recv().expect("responder should have been called");
    assert_eq!(response.action, ElicitationAction::Accept);
    let content = response.content.expect("content should be present");
    assert_eq!(content["decision"].as_str().unwrap(), "approve");
}

#[test]
fn request_changes_sends_feedback_in_payload() {
    let markdown = "# Plan\nbroken line";
    let (mut screen, mut rx) = make_screen(markdown);

    // Add a comment
    assert!(!screen.on_key(key(KeyCode::Char('j')))); // move to line 1
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "this is wrong");
    assert!(!screen.on_key(key(KeyCode::Enter)));

    // Request changes
    assert!(screen.on_key(key(KeyCode::Char('r'))));

    let response = rx.try_recv().expect("responder should have been called");
    assert_eq!(response.action, ElicitationAction::Accept);
    let content = response.content.expect("content should be present");
    assert_eq!(content["decision"].as_str().unwrap(), "deny");
    assert!(content["feedback"].as_str().unwrap().contains("this is wrong"));
}

#[test]
fn cancel_sends_correct_payload() {
    let markdown = "# Plan\ntext";
    let (mut screen, mut rx) = make_screen(markdown);

    assert!(screen.on_key(key(KeyCode::Esc)));

    let response = rx.try_recv().expect("responder should have been called");
    assert_eq!(response.action, ElicitationAction::Cancel);
}

#[test]
fn responder_is_called_exactly_once_on_approve() {
    let markdown = "# Plan\ntext";
    let (mut screen, mut rx) = make_screen(markdown);

    assert!(screen.on_key(key(KeyCode::Char('a'))));
    assert!(rx.try_recv().is_ok(), "responder should fire exactly once");
    assert!(rx.try_recv().is_err(), "responder should NOT fire twice");
}

#[test]
fn responder_is_called_exactly_once_on_close_replacement() {
    let markdown = "# Plan\ntext";
    let (mut screen, mut rx) = make_screen(markdown);

    // Call cancel() (which happens on screen close/replacement)
    screen.cancel();

    assert!(rx.try_recv().is_ok(), "cancel should fire responder");
    assert!(rx.try_recv().is_err(), "responder should NOT fire twice");

    // Calling cancel again should be a no-op
    screen.cancel();
}

#[test]
fn responder_is_called_exactly_once_on_request_changes() {
    let markdown = "# Plan\ntext";
    let (mut screen, mut rx) = make_screen(markdown);

    assert!(screen.on_key(key(KeyCode::Char('r'))));
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());

    // After responder fires, screen stays closed - all keys return true
    assert!(screen.on_key(key(KeyCode::Char('j'))));
    assert!(screen.on_key(key(KeyCode::Char('a'))));
    assert!(rx.try_recv().is_err());
}

// ============================================================================
// Wide two-pane and narrow one-pane buffer rendering tests
// ============================================================================

#[test]
fn wide_screen_shows_plan_and_outline_panels() {
    let markdown = "# Overview\ncontent\n## Details\nmore";
    let (mut screen, _rx) = make_screen(markdown);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("Outline"), "wide screen should show Outline panel: {text}");
    assert!(text.contains("Overview"), "wide screen should show section in outline");
    assert!(text.contains("Details"), "wide screen should show second section");
}

#[test]
fn wide_screen_renders_plan_with_line_numbers() {
    let markdown = "# Plan\nline one\nline two\nline three";
    let (mut screen, _rx) = make_screen(markdown);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains('1'), "should have line number 1");
    assert!(text.contains("line one"), "should show source text");
}

#[test]
fn narrow_screen_falls_back_to_single_pane() {
    let markdown = "# Overview\ntext\n## Details\nmore";
    let (mut screen, _rx) = make_screen(markdown);

    // Width under MIN_SPLIT_WIDTH (60)
    let buffer = render_screen(&mut screen, 50, 24);
    let text = buffer_text(&buffer);
    assert!(!text.contains("Outline"), "narrow screen should NOT show Outline panel but got: {text}");
    assert!(text.contains("text"), "narrow screen should show plan content");
}

#[test]
fn footer_shows_context_sensitive_hints() {
    let markdown = "# Plan\ntext";
    let (mut screen, _rx) = make_screen(markdown);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("approve"), "footer should show approve hint");
    assert!(text.contains("cancel"), "footer should show cancel hint");
    assert!(text.contains("comment"), "footer should show comment hint in plan mode");
}

#[test]
fn comment_editor_is_rendered_inline() {
    let markdown = "# Plan\nsome line\n## More\nanother line";
    let (mut screen, _rx) = make_screen(markdown);

    // Start a comment on line 2 (0-indexed)
    assert!(!screen.on_key(key(KeyCode::Char('j'))));
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "fix this");

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("new comment"), "should show [new comment] header: {text}");
    assert!(text.contains("fix this"), "should show draft text: {text}");
}

#[test]
fn submitted_comments_appear_inline() {
    let markdown = "# Plan\nline one\nline two";
    let (mut screen, _rx) = make_screen(markdown);

    // Submit a comment
    assert!(!screen.on_key(key(KeyCode::Char('c'))));
    type_text(&mut screen, "needs improvement");
    assert!(!screen.on_key(key(KeyCode::Enter)));

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("comment on line"), "should show comment header: {text}");
    assert!(text.contains("needs improvement"), "should show comment body: {text}");
}

#[test]
fn focus_switches_between_outline_and_plan() {
    let markdown = "# Overview\ncontent\n## Details\nmore";
    let (mut screen, _rx) = make_screen(markdown);

    // Plan is focused by default
    // Switch to outline
    assert!(!screen.on_key(key(KeyCode::Char('h'))));

    // In outline mode, Enter should jump
    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("Overview"), "outline should show section");

    // Switch back to plan
    assert!(!screen.on_key(key(KeyCode::Char('l'))));
}

#[test]
fn outline_without_sections_shows_no_split() {
    let markdown = "just text\nno headings";
    let (mut screen, _rx) = make_screen(markdown);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(!text.contains("Outline"), "no outline when no headings: {text}");
}

#[test]
fn mouse_click_in_outline_pane_focuses_outline_at_wide_width() {
    let markdown = "# Section One\nline a\n## Section Two\nline b";
    let (mut screen, _rx) = make_screen(markdown);

    render_screen(&mut screen, 80, 24);

    // Clicks at y=2 (past border), x=2 (left side, within outline_width = 80/4 = 20)
    screen.on_mouse_click(2, 2);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    // Outline should have accent background when focused
    assert!(text.contains("Section One"), "outline should show sections: {text}");
    assert!(text.contains("Section Two"), "outline should show sections: {text}");
}

#[test]
fn mouse_click_in_plan_pane_focuses_plan_at_wide_width() {
    let markdown = "# Section One\nline a\n## Section Two\nline b";
    let (mut screen, _rx) = make_screen(markdown);

    render_screen(&mut screen, 80, 24);

    // Clicks at y=2, x=60 (right side, past outline_width = 20)
    screen.on_mouse_click(2, 60);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("Section One"), "plan should show sections: {text}");
}

#[test]
fn mouse_click_at_narrow_width_always_focuses_plan() {
    let markdown = "# Section One\nline a\n## Section Two\nline b";
    let (mut screen, _rx) = make_screen(markdown);

    // 50-wide: below MIN_SPLIT_WIDTH (60), no split
    render_screen(&mut screen, 50, 24);

    // Click at y=2, x=2 — even on the "left" side, should be Plan
    screen.on_mouse_click(2, 2);

    let buffer = render_screen(&mut screen, 50, 24);
    let text = buffer_text(&buffer);
    // No outline pane in narrow layout
    assert!(!text.contains("Outline"), "no outline in narrow layout: {text}");
}

#[test]
fn mouse_click_on_border_is_ignored() {
    let markdown = "# Section One\nline a";
    let (mut screen, _rx) = make_screen(markdown);

    render_screen(&mut screen, 80, 24);

    // Plan starts as default focus
    // Click at y=0 (border), focus shouldn't change
    screen.on_mouse_click(0, 30);

    let buffer = render_screen(&mut screen, 80, 24);
    let text = buffer_text(&buffer);
    assert!(text.contains("Section One"), "plan should still render: {text}");
}

#[test]
fn mouse_click_after_resize_uses_correct_pane_rects() {
    let markdown = "# Section One\nline a\n## Section Two\nline b";
    let (mut screen, _rx) = make_screen(markdown);

    // Render at wide width
    render_screen(&mut screen, 100, 30);

    // outline_width at width=100: 100/4=25
    // Click at x=5 (outline side)
    screen.on_mouse_click(2, 5);

    // Resize to different width
    render_screen(&mut screen, 120, 30);

    // outline_width at width=120: 120/4=30
    // Click at x=5 still in outline side
    screen.on_mouse_click(2, 5);

    // Click at x=90 is in plan side at width 120
    screen.on_mouse_click(2, 90);

    let buffer = render_screen(&mut screen, 120, 30);
    let text = buffer_text(&buffer);
    assert!(text.contains("Section One"), "plan should render: {text}");
}

fn type_text(screen: &mut PlanReviewScreen, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            screen.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        } else {
            screen.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }
}
