use std::mem::{Discriminant, discriminant};

use crate::components::thought_message::ThoughtMessage;
use crate::components::tool_call_statuses::ToolCallStatuses;
use tui::{FitOptions, Frame, Insets, Line, Style, ViewContext, render_markdown_result};

#[derive(Debug, Clone)]
pub enum SegmentContent {
    UserMessage(String),
    Text(String),
    Thought(String),
    ToolCall(String),
}

#[derive(Debug)]
struct Segment {
    content: SegmentContent,
}

#[doc = include_str!("../docs/conversation_window.md")]
pub struct ConversationBuffer {
    segments: Vec<Segment>,
    thought_block_open: bool,
}

impl Default for ConversationBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationBuffer {
    pub fn new() -> Self {
        Self { segments: Vec::new(), thought_block_open: false }
    }

    #[cfg(test)]
    pub(crate) fn segments(&self) -> impl ExactSizeIterator<Item = &SegmentContent> {
        self.segments.iter().map(|s| &s.content)
    }

    pub fn push_user_message(&mut self, text: &str) {
        self.close_thought_block();
        self.segments.push(Segment { content: SegmentContent::UserMessage(text.to_string()) });
    }

    pub fn append_text_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }

        self.close_thought_block();

        if let Some(segment) = self.segments.last_mut()
            && let SegmentContent::Text(existing) = &mut segment.content
        {
            existing.push_str(chunk);
        } else {
            self.segments.push(Segment { content: SegmentContent::Text(chunk.to_string()) });
        }
    }

    pub fn append_thought_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }

        if self.thought_block_open
            && let Some(segment) = self.segments.last_mut()
            && let SegmentContent::Thought(existing) = &mut segment.content
        {
            existing.push_str(chunk);
            return;
        }

        self.segments.push(Segment { content: SegmentContent::Thought(chunk.to_string()) });
        self.thought_block_open = true;
    }

    pub(crate) fn close_thought_block(&mut self) {
        self.thought_block_open = false;
    }

    pub(crate) fn clear(&mut self) {
        self.segments.clear();
        self.thought_block_open = false;
    }

    pub(crate) fn ensure_tool_segment(&mut self, tool_id: &str) {
        let has_segment =
            self.segments.iter().any(|s| matches!(&s.content, SegmentContent::ToolCall(id) if id == tool_id));

        if !has_segment {
            self.segments.push(Segment { content: SegmentContent::ToolCall(tool_id.to_string()) });
        }
    }

    #[cfg(test)]
    fn drain_segments_except(&mut self, mut keep: impl FnMut(&SegmentContent) -> bool) -> Vec<Segment> {
        let old = std::mem::take(&mut self.segments);
        let (kept, removed) = old.into_iter().partition(|s| keep(&s.content));
        self.segments = kept;
        removed
    }

    #[cfg(test)]
    pub(crate) fn drain_completed(
        &mut self,
        tool_call_statuses: &ToolCallStatuses,
    ) -> (Vec<SegmentContent>, Vec<String>) {
        let drained = self.drain_segments_except(
            |seg| matches!(seg, SegmentContent::ToolCall(id) if tool_call_statuses.is_tool_running(id)),
        );

        let mut content = Vec::new();
        let mut completed_tool_ids = Vec::new();

        for segment in drained {
            if let SegmentContent::ToolCall(ref id) = segment.content {
                completed_tool_ids.push(id.clone());
            }
            content.push(segment.content);
        }

        (content, completed_tool_ids)
    }
}

pub struct ConversationWindow<'a> {
    pub conversation: &'a ConversationBuffer,
    pub tool_call_statuses: &'a ToolCallStatuses,
    pub content_padding: usize,
}

impl ConversationWindow<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        let pad_u16 = u16::try_from(self.content_padding).unwrap_or(u16::MAX);
        let content_ctx = context.inset(Insets::horizontal(pad_u16));

        let mut sections: Vec<Frame> = Vec::new();
        let mut last_segment_kind: Option<Discriminant<SegmentContent>> = None;

        for segment in &self.conversation.segments {
            let kind = discriminant(&segment.content);
            let frame = if matches!(segment.content, SegmentContent::UserMessage(_)) {
                render_segment_frame(&segment.content, self.tool_call_statuses, self.content_padding, context)
            } else {
                render_segment_frame(&segment.content, self.tool_call_statuses, self.content_padding, &content_ctx)
                    .indent(pad_u16)
            };

            if frame.lines().is_empty() {
                continue;
            }

            if let Some(prev_kind) = last_segment_kind
                && prev_kind != kind
            {
                sections.push(Frame::new(vec![Line::default()]));
            }
            sections.push(frame);
            last_segment_kind = Some(kind);
        }

        Frame::vstack(sections)
    }
}

fn render_segment_frame(
    segment: &SegmentContent,
    tool_call_statuses: &ToolCallStatuses,
    content_padding: usize,
    context: &ViewContext,
) -> Frame {
    match segment {
        SegmentContent::UserMessage(text) => Frame::new(render_user_message_block(text, content_padding, context)),
        SegmentContent::Thought(text) => ThoughtMessage { text }.render(context),
        SegmentContent::Text(text) => {
            Frame::new(render_markdown_result(text, context).to_lines()).fit(context.size.width, FitOptions::wrap())
        }
        SegmentContent::ToolCall(id) => tool_call_statuses.render_tool(id, context),
    }
}

fn render_user_message_block(text: &str, content_padding: usize, context: &ViewContext) -> Vec<Line> {
    if text.is_empty() {
        return vec![];
    }

    let block_style = Style::fg(context.theme.text_primary()).bg_color(context.theme.sidebar_bg());
    let block_width = usize::from(context.size.width).max(1);
    let left_padding = content_padding.min(block_width.saturating_sub(1));
    let mut rendered_lines = Vec::new();
    rendered_lines.push(padded_background_line(block_width, block_style));

    for content in text.lines() {
        rendered_lines.extend(render_user_message_lines(content, left_padding, block_width, block_style));
    }

    rendered_lines.push(padded_background_line(block_width, block_style));
    rendered_lines
}

fn render_user_message_lines(content: &str, left_padding: usize, block_width: usize, block_style: Style) -> Vec<Line> {
    if content.is_empty() {
        return vec![padded_background_line(block_width, block_style)];
    }

    let content_width = block_width.saturating_sub(left_padding).max(1);
    Line::with_style(content.to_string(), block_style)
        .soft_wrap(u16::try_from(content_width).unwrap_or(u16::MAX))
        .into_iter()
        .map(|line| pad_user_message_line(&line, left_padding, block_width, block_style))
        .collect()
}

fn pad_user_message_line(line: &Line, left_padding: usize, block_width: usize, block_style: Style) -> Line {
    let mut padded_line = Line::with_style(" ".repeat(left_padding), block_style);
    padded_line.append_line(line);

    let trailing_padding = block_width.saturating_sub(padded_line.display_width());
    if trailing_padding > 0 {
        padded_line.push_with_style(" ".repeat(trailing_padding), block_style);
    }

    padded_line
}

fn padded_background_line(width: usize, style: Style) -> Line {
    Line::with_style(" ".repeat(width.max(1)), style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_CONTENT_PADDING;

    #[test]
    fn buffer_closes_thought_block_when_text_arrives() {
        let mut buffer = ConversationBuffer::new();
        buffer.append_thought_chunk("thinking");
        buffer.append_text_chunk("answer");
        buffer.append_thought_chunk("new thought");

        let segments: Vec<_> = buffer.segments().collect();
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[0], SegmentContent::Thought(_)));
        assert!(matches!(segments[1], SegmentContent::Text(_)));
        assert!(matches!(segments[2], SegmentContent::Thought(_)));
    }

    #[test]
    fn buffer_coalesces_contiguous_thought_chunks() {
        let mut buffer = ConversationBuffer::new();
        buffer.append_thought_chunk("a");
        buffer.append_thought_chunk("b");

        let segments: Vec<_> = buffer.segments().collect();
        assert_eq!(segments.len(), 1);
        match segments[0] {
            SegmentContent::Thought(text) => assert_eq!(text, "ab"),
            _ => panic!("expected thought segment"),
        }
    }

    #[test]
    fn clear_removes_segments_and_resets_state() {
        let mut buffer = ConversationBuffer::new();
        buffer.append_thought_chunk("thinking");
        buffer.append_text_chunk("answer");
        assert_eq!(buffer.segments().len(), 2);

        buffer.clear();

        assert_eq!(buffer.segments().len(), 0);
        buffer.append_thought_chunk("new");
        assert_eq!(buffer.segments().len(), 1);
    }

    #[test]
    fn user_message_renders_with_top_and_bottom_padding_lines() {
        let mut buffer = ConversationBuffer::new();
        buffer.push_user_message("hello");

        let tool_call_statuses = ToolCallStatuses::new();
        let window = ConversationWindow {
            conversation: &buffer,
            tool_call_statuses: &tool_call_statuses,
            content_padding: DEFAULT_CONTENT_PADDING,
        };
        let context = ViewContext::new((80, 24));

        let frame = window.render(&context);
        let lines = frame.lines();

        assert_eq!(lines.len(), 3);
        let left_padding = " ".repeat(DEFAULT_CONTENT_PADDING);
        assert_eq!(lines[1].plain_text().trim_end(), format!("{left_padding}hello"));
        assert!(lines[0].plain_text().trim().is_empty());
        assert!(lines[2].plain_text().trim().is_empty());
        assert_user_message_style(&lines[0], &context);
        assert_user_message_style(&lines[1], &context);
        assert_user_message_style(&lines[2], &context);
        assert!(lines.iter().all(|line| line.display_width() == usize::from(context.size.width)));
    }

    #[test]
    fn user_message_block_applies_theme_bg_to_all_lines() {
        let mut buffer = ConversationBuffer::new();
        buffer.push_user_message("line one\n\nline three");

        let tool_call_statuses = ToolCallStatuses::new();
        let window = ConversationWindow {
            conversation: &buffer,
            tool_call_statuses: &tool_call_statuses,
            content_padding: DEFAULT_CONTENT_PADDING,
        };
        let context = ViewContext::new((80, 24));

        let frame = window.render(&context);
        let lines = frame.lines();

        assert_eq!(lines.len(), 5);
        let left_padding = " ".repeat(DEFAULT_CONTENT_PADDING);
        assert_eq!(lines[1].plain_text().trim_end(), format!("{left_padding}line one"));
        assert!(lines[2].plain_text().trim().is_empty());
        assert_eq!(lines[3].plain_text().trim_end(), format!("{left_padding}line three"));

        for line in lines {
            assert_user_message_style(line, &context);
        }

        let first_width = lines[0].display_width();
        assert_eq!(first_width, usize::from(context.size.width));
        assert!(lines.iter().all(|line| line.display_width() == first_width));
    }

    #[test]
    fn user_message_wrapped_rows_keep_full_width_background() {
        let mut buffer = ConversationBuffer::new();
        buffer.push_user_message("0123456789");

        let tool_call_statuses = ToolCallStatuses::new();
        let window = ConversationWindow {
            conversation: &buffer,
            tool_call_statuses: &tool_call_statuses,
            content_padding: DEFAULT_CONTENT_PADDING,
        };
        let context = ViewContext::new((8, 24));

        let frame = window.render(&context);
        let lines = frame.lines();

        let pad = " ".repeat(DEFAULT_CONTENT_PADDING);
        let content_width = 8 - DEFAULT_CONTENT_PADDING;
        let expected_lines = 2 + "0123456789".len().div_ceil(content_width);
        assert_eq!(lines.len(), expected_lines);
        for line in &lines[1..lines.len() - 1] {
            assert!(line.plain_text().starts_with(&pad), "line should start with padding: '{}'", line.plain_text());
        }
        assert!(lines.iter().all(|line| line.display_width() == usize::from(context.size.width)));
        for line in lines {
            assert_user_message_style(line, &context);
        }
    }

    #[test]
    fn drain_completed_returns_content_and_tool_ids() {
        use agent_client_protocol::schema as acp;

        let mut buffer = ConversationBuffer::new();
        buffer.append_text_chunk("hello");
        buffer.ensure_tool_segment("tool-1");

        let mut statuses = ToolCallStatuses::new();
        let tc = acp::ToolCall::new("tool-1", "Read file");
        statuses.on_tool_call(&tc);
        let update =
            acp::ToolCallUpdate::new("tool-1", acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed));
        statuses.on_tool_call_update(&update);

        let (content, tool_ids) = buffer.drain_completed(&statuses);

        assert_eq!(content.len(), 2, "should have text and tool content");
        assert!(matches!(content[0], SegmentContent::Text(_)));
        assert!(matches!(content[1], SegmentContent::ToolCall(_)));
        assert_eq!(tool_ids, vec!["tool-1"]);
        assert_eq!(buffer.segments().len(), 0, "all segments should be drained");
    }

    #[test]
    fn drain_completed_keeps_running_tools() {
        use agent_client_protocol::schema as acp;

        let mut buffer = ConversationBuffer::new();
        buffer.append_text_chunk("hello");
        buffer.ensure_tool_segment("tool-1");

        let mut statuses = ToolCallStatuses::new();
        let tc = acp::ToolCall::new("tool-1", "Read file");
        statuses.on_tool_call(&tc);
        // tool-1 stays Running (no completion update)

        let (content, tool_ids) = buffer.drain_completed(&statuses);

        assert_eq!(content.len(), 1, "text segment should still be drained");
        assert!(matches!(content[0], SegmentContent::Text(_)));
        assert!(tool_ids.is_empty(), "running tool should not be drained");
        let segments: Vec<_> = buffer.segments().collect();
        assert_eq!(segments.len(), 1, "running tool should remain");
        assert!(matches!(
            segments[0],
            SegmentContent::ToolCall(id) if id == "tool-1"
        ));
    }

    #[test]
    fn agent_text_continuation_lines_have_padding() {
        let mut buffer = ConversationBuffer::new();
        buffer.append_text_chunk("abcdefghijklmnopqrstuvwx");

        let tool_call_statuses = ToolCallStatuses::new();
        let window = ConversationWindow {
            conversation: &buffer,
            tool_call_statuses: &tool_call_statuses,
            content_padding: DEFAULT_CONTENT_PADDING,
        };
        let context = ViewContext::new((20, 24));

        let frame = window.render(&context);
        let lines = frame.lines();
        let padding_prefix = " ".repeat(DEFAULT_CONTENT_PADDING);
        assert!(lines.len() >= 2, "text should wrap into at least 2 lines, got {}", lines.len());
        for (i, line) in lines.iter().enumerate() {
            let text = line.plain_text();
            assert!(text.starts_with(&padding_prefix), "line {i} should start with padding: '{text}'");
            assert!(
                line.display_width() <= usize::from(context.size.width),
                "line {i} should not exceed terminal width: width={}, max={}",
                line.display_width(),
                context.size.width
            );
        }
    }

    fn assert_user_message_style(line: &Line, context: &ViewContext) {
        assert!(!line.spans().is_empty());
        assert!(line.spans().iter().all(|span| span.style().bg == Some(context.theme.sidebar_bg())));
        assert!(line.spans().iter().all(|span| span.style().fg == Some(context.theme.text_primary())));
    }
}
