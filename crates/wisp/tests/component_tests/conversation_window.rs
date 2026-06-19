use tui::ViewContext;
use tui::testing::render_lines;
use wisp::components::conversation_window::{ConversationBuffer, ConversationWindow};
use wisp::components::tool_call_statuses::ToolCallStatuses;
use wisp::settings::DEFAULT_CONTENT_PADDING;

#[test]
fn renders_empty_when_no_segments() {
    let conversation = ConversationBuffer::new();
    let statuses = ToolCallStatuses::new();
    let context = ViewContext::new((80, 24));
    let view = ConversationWindow {
        conversation: &conversation,
        tool_call_statuses: &statuses,
        content_padding: DEFAULT_CONTENT_PADDING,
    };

    let frame = view.render(&context);
    assert!(frame.lines().is_empty());
}

#[test]
fn inserts_vertical_margin_between_different_segment_kinds() {
    let mut conversation = ConversationBuffer::new();
    conversation.append_text_chunk("one");
    conversation.append_thought_chunk("two");
    conversation.append_text_chunk("three");
    let statuses = ToolCallStatuses::new();
    let context = ViewContext::new((80, 24));
    let view = ConversationWindow {
        conversation: &conversation,
        tool_call_statuses: &statuses,
        content_padding: DEFAULT_CONTENT_PADDING,
    };

    let lines = view.render(&context).into_lines();
    assert_eq!(lines.len(), 5);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].starts_with(&" ".repeat(DEFAULT_CONTENT_PADDING)), "text should be padded: {}", output[0]);
    assert!(output[0].contains("one"));
    assert_eq!(output[1], "");
    assert!(output[2].starts_with(&" ".repeat(DEFAULT_CONTENT_PADDING)), "thought should be padded: {}", output[2]);
    assert!(output[2].contains("two"));
    assert_eq!(output[3], "");
    assert!(output[4].starts_with(&" ".repeat(DEFAULT_CONTENT_PADDING)), "text should be padded: {}", output[4]);
    assert!(output[4].contains("three"));
}

#[test]
fn consecutive_text_chunks_render_without_margin() {
    let mut conversation = ConversationBuffer::new();
    conversation.append_text_chunk("first ");
    conversation.append_text_chunk("second");
    let statuses = ToolCallStatuses::new();
    let context = ViewContext::new((80, 24));
    let view = ConversationWindow {
        conversation: &conversation,
        tool_call_statuses: &statuses,
        content_padding: DEFAULT_CONTENT_PADDING,
    };

    let lines = view.render(&context).into_lines();
    // Consecutive text chunks are coalesced, so there should be one line with no margin
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].starts_with(&" ".repeat(DEFAULT_CONTENT_PADDING)), "text should be padded: {}", output[0]);
    assert!(output[0].contains("first"), "text: {}", output[0]);
    assert!(output[0].contains("second"), "text: {}", output[0]);
}

#[test]
fn wrapped_agent_text_has_padding_on_all_lines() {
    let mut conversation = ConversationBuffer::new();
    conversation.append_text_chunk("abcdefghijklmnopqrstuvwx");
    let statuses = ToolCallStatuses::new();
    let width: u16 = 20;
    let context = ViewContext::new((width, 24));
    let view = ConversationWindow {
        conversation: &conversation,
        tool_call_statuses: &statuses,
        content_padding: DEFAULT_CONTENT_PADDING,
    };

    let lines = view.render(&context).into_lines();
    let term = render_lines(&lines, width, 24);
    let output = term.get_lines();
    let padding = " ".repeat(DEFAULT_CONTENT_PADDING);
    let content_lines: Vec<_> = output.iter().filter(|l| !l.trim().is_empty()).collect();
    assert!(content_lines.len() >= 2, "should wrap into at least 2 lines, got {}", content_lines.len());
    for (i, line) in content_lines.iter().enumerate() {
        assert!(line.starts_with(&padding), "line {i} should start with padding: '{line}'");
    }
}
