use tui::testing::{TestTerminal, assert_buffer_eq};
use tui::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::common::*;

fn expected_multiline_prompt(width: u16, lines: &[&str]) -> Vec<String> {
    let rule = "─".repeat(width as usize);
    let mut expected = Vec::with_capacity(lines.len() + 3);
    expected.push(rule.clone());
    for (index, line) in lines.iter().enumerate() {
        let prefix = if index == 0 { "> " } else { "  " };
        expected.push(format!("{prefix}{line}").trim_end().to_string());
    }
    expected.push(rule);
    expected.push(expected_status_line(width, TEST_AGENT));
    expected
}

#[tokio::test]
async fn test_user_message_submission() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "Hello world").await;
    press_enter(&mut renderer).await;

    // Simulate the agent finishing so the grid loader clears
    renderer.on_prompt_done().unwrap();

    let expected = expected_with_prompt(&["", &p("Hello world"), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_typing_renders_within_bordered_input() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;

    let expected = expected_prompt(80, "hello", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_backspace_updates_within_border() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;
    press_backspace(&mut renderer).await;

    let expected = expected_prompt(80, "hell", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_wrapped_input_prompt_rerender_has_single_prompt() {
    let terminal = TestTerminal::new(32, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (32, 24));

    renderer.initial_render().unwrap();
    type_string(&mut renderer, "this input prompt is long enough to wrap across multiple rows").await;
    press_backspace(&mut renderer).await;
    press_backspace(&mut renderer).await;

    let lines = renderer.writer().get_lines();
    let rule = "─".repeat(32);
    let rule_count = lines.iter().filter(|l| **l == rule).count();
    let content_rows = lines.iter().filter(|l| l.starts_with('>') || l.starts_with("  ")).count();

    assert_eq!(
        rule_count,
        2,
        "Expected exactly two horizontal rules after wrapped rerender.\nBuffer:\n{}",
        lines.join("\n")
    );
    assert!(content_rows >= 2, "Expected wrapped prompt content rows.\nBuffer:\n{}", lines.join("\n"));
}

#[tokio::test]
async fn test_resize_after_terminal_reflow_keeps_single_prompt() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    let input = "this input prompt is long enough to wrap across multiple rows and should reflow cleanly on resize";
    type_string(&mut renderer, input).await;

    renderer.test_writer_mut().resize_preserving_transcript(32, 24);
    renderer.on_resize_event(32, 24).await.unwrap();

    let lines = renderer.writer().get_lines();
    let rule = "─".repeat(32);
    let rule_count = lines.iter().filter(|l| **l == rule).count();
    let content_rows = lines.iter().filter(|l| l.starts_with('>') || l.starts_with("  ")).count();

    assert_eq!(
        rule_count,
        2,
        "Expected exactly two horizontal rules after resize reflow.\nBuffer:\n{}",
        lines.join("\n")
    );
    assert!(
        content_rows >= 2,
        "Expected wrapped prompt content rows after resize reflow.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_empty_prompt_renders_bordered_box() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));

    renderer.initial_render().unwrap();

    let expected = expected_prompt(80, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_paste_inserts_all_text_at_once() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer.on_paste("hello world").await.unwrap();

    let expected = expected_prompt(80, "hello world", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_paste_strips_control_characters() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer.on_paste("line1\nline2\ttab").await.unwrap();

    let expected = expected_prompt(80, "line1line2tab", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_paste_closes_file_picker() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    // Open file picker with @
    renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('@'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();
    assert!(has_file_picker(renderer.writer()), "File picker should be open");

    // Paste should close the picker and append text
    renderer.on_paste("pasted text").await.unwrap();

    assert!(!has_file_picker(renderer.writer()), "File picker should be closed");
    let expected = expected_prompt(80, "@pasted text", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_cursor_position_after_whitespace_wrap() {
    let width: u16 = 12;
    let terminal = TestTerminal::new(width, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (width, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "abc def ghi").await;

    let rule = "─".repeat(width as usize);
    let expected = vec![
        rule.clone(),
        "> abc def".to_string(),
        "  ghi".to_string(),
        rule.clone(),
        expected_status_line(width, TEST_AGENT),
    ];
    assert_buffer_eq(renderer.writer(), &expected);

    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!(cursor_row, 2, "cursor should be on the second content row (row 2)");
    assert_eq!(cursor_col, 5, "cursor should be at col 5 (2 prefix + 3 for 'ghi')");

    type_string(&mut renderer, "j").await;

    let expected_after = vec![
        rule.clone(),
        "> abc def".to_string(),
        "  ghij".to_string(),
        rule,
        expected_status_line(width, TEST_AGENT),
    ];
    assert_buffer_eq(renderer.writer(), &expected_after);

    let (cursor_col_after, cursor_row_after) = renderer.writer().cursor_position();
    assert_eq!(cursor_row_after, 2, "cursor should stay on the second content row after typing 'j'");
    assert_eq!(cursor_col_after, 6, "cursor should advance to col 6 (2 prefix + 4 for 'ghij')");
}

#[tokio::test]
async fn test_cursor_position_after_multi_mention_wrap() {
    let width: u16 = 12;
    let terminal = TestTerminal::new(width, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (width, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "@aaaaa @bbbbbb").await;

    let rule = "─".repeat(width as usize);
    let lines = renderer.writer().get_lines();
    let expected_prompt = vec![rule.clone(), "> @aaaaa".to_string(), "  @bbbbbb".to_string(), rule];
    assert_eq!(&lines[..4], expected_prompt.as_slice());

    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!(cursor_row, 2, "cursor should be on the second content row (row 2)");
    assert_eq!(cursor_col, 9, "cursor should be at col 9 (2 prefix + 7 for '@bbbbbb')");
}

#[tokio::test]
async fn test_shift_enter_creates_hard_line_break() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "line one").await;
    press_shift_enter(&mut renderer).await;
    type_string(&mut renderer, "line two").await;

    let expected = expected_multiline_prompt(80, &["line one", "line two"]);
    assert_buffer_eq(renderer.writer(), &expected);

    let (cursor_col, cursor_row) = renderer.writer().cursor_position();
    assert_eq!(cursor_row, 2);
    assert_eq!(cursor_col, 10);
}

#[tokio::test]
async fn test_alt_enter_creates_hard_line_break() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;
    send_key(&mut renderer, KeyCode::Enter, KeyModifiers::ALT).await;

    let expected = expected_multiline_prompt(80, &["hello", ""]);
    assert_buffer_eq(renderer.writer(), &expected);
    assert_eq!(renderer.writer().cursor_position(), (2, 2));
}

#[tokio::test]
async fn test_alt_enter_closes_command_picker_and_inserts_newline() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/").await;
    assert!(has_command_picker(renderer.writer()));
    send_key(&mut renderer, KeyCode::Enter, KeyModifiers::ALT).await;

    let expected = expected_multiline_prompt(80, &["/", ""]);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_up_arrow_moves_cursor_across_hard_newline() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "line one").await;
    press_shift_enter(&mut renderer).await;
    type_string(&mut renderer, "line two").await;
    press_up(&mut renderer).await;
    type_string(&mut renderer, "!").await;

    let expected = expected_multiline_prompt(80, &["line one!", "line two"]);
    assert_buffer_eq(renderer.writer(), &expected);
    assert_eq!(renderer.writer().cursor_position(), (11, 1));
}

#[tokio::test]
async fn test_down_arrow_moves_cursor_across_hard_newline() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "line one").await;
    press_shift_enter(&mut renderer).await;
    type_string(&mut renderer, "line two").await;
    press_up(&mut renderer).await;
    press_down(&mut renderer).await;
    type_string(&mut renderer, "!").await;

    let expected = expected_multiline_prompt(80, &["line one", "line two!"]);
    assert_buffer_eq(renderer.writer(), &expected);
    assert_eq!(renderer.writer().cursor_position(), (11, 2));
}

#[tokio::test]
async fn test_ctrl_a_and_ctrl_e_move_within_hard_newline() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "line one").await;
    press_shift_enter(&mut renderer).await;
    type_string(&mut renderer, "line two").await;

    send_key(&mut renderer, KeyCode::Char('a'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "[").await;
    send_key(&mut renderer, KeyCode::Char('e'), KeyModifiers::CONTROL).await;
    type_string(&mut renderer, "]").await;

    let expected = expected_multiline_prompt(80, &["line one", "[line two]"]);
    assert_buffer_eq(renderer.writer(), &expected);
    assert_eq!(renderer.writer().cursor_position(), (12, 2));
}
