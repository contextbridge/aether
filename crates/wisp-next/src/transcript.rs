use crate::tool_calls::ToolCallLog;

/// One unit of conversation content, in transcript order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentContent {
    UserMessage(String),
    Text(String),
    Thought(String),
    ToolCall(String),
}

/// Ordered conversation history with streaming append semantics.
///
/// Streaming chunks coalesce into the trailing segment; completed segments are
/// handed off to presentation state via [`Transcript::drain_finalized_prefix`].
#[derive(Debug, Default)]
pub struct Transcript {
    segments: Vec<SegmentContent>,
    thought_block_open: bool,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending(&self) -> &[SegmentContent] {
        &self.segments
    }

    pub fn push_user_message(&mut self, text: &str) {
        self.close_thought_block();
        self.segments.push(SegmentContent::UserMessage(text.to_string()));
    }

    pub fn append_text_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }

        self.close_thought_block();

        if let Some(SegmentContent::Text(existing)) = self.segments.last_mut() {
            existing.push_str(chunk);
        } else {
            self.segments.push(SegmentContent::Text(chunk.to_string()));
        }

        let SegmentContent::Text(text) = self.segments.last_mut().expect("text segment was just appended") else {
            unreachable!();
        };
        if let Some(finalized_end) = last_finalizable_offset(text) {
            let trailing = text.split_off(finalized_end);
            self.segments.push(SegmentContent::Text(trailing));
        }
    }

    pub fn append_thought_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }

        if self.thought_block_open
            && let Some(SegmentContent::Thought(existing)) = self.segments.last_mut()
        {
            existing.push_str(chunk);
            return;
        }

        self.segments.push(SegmentContent::Thought(chunk.to_string()));
        self.thought_block_open = true;
    }

    pub fn close_thought_block(&mut self) {
        self.thought_block_open = false;
    }

    pub fn clear(&mut self) {
        self.segments.clear();
        self.thought_block_open = false;
    }

    pub fn ensure_tool_segment(&mut self, tool_id: &str) {
        let has_segment = self.segments.iter().any(|s| matches!(s, SegmentContent::ToolCall(id) if id == tool_id));

        if !has_segment {
            self.segments.push(SegmentContent::ToolCall(tool_id.to_string()));
        }
    }

    /// Remove and return the longest prefix of segments that can never mutate
    /// again, so presentation state can take ownership exactly once.
    ///
    /// A segment is final when it is a user message, a completed tool call, or
    /// streamed text/thought content that is no longer the trailing segment of
    /// an in-flight prompt. Text segments are split after completed lines
    /// outside fenced code blocks so finalized content can move independently
    /// of the trailing streaming line.
    pub fn drain_finalized_prefix(&mut self, tool_calls: &ToolCallLog, prompt_in_flight: bool) -> Vec<SegmentContent> {
        let final_len = self
            .segments
            .iter()
            .enumerate()
            .take_while(|(index, segment)| match segment {
                SegmentContent::UserMessage(_) => true,
                SegmentContent::Text(_) | SegmentContent::Thought(_) => {
                    *index + 1 < self.segments.len() || !prompt_in_flight
                }
                SegmentContent::ToolCall(id) => !tool_calls.is_running(id),
            })
            .count();

        self.segments.drain(..final_len).collect()
    }
}

/// Byte offset just past the last completed line that leaves no code fence
/// open. Segments must never split inside a fenced block: each segment is
/// rendered as an independent markdown document, so a fence split across
/// segments would lose its language context and syntax highlighting.
fn last_finalizable_offset(text: &str) -> Option<usize> {
    let mut open_fence: Option<(char, usize)> = None;
    let mut finalizable = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if !line.ends_with('\n') {
            break;
        }
        match open_fence {
            None => match fence_delimiter(line) {
                Some((fence_char, length, _)) => open_fence = Some((fence_char, length)),
                None => finalizable = Some(offset),
            },
            Some((fence_char, length)) => {
                if let Some((close_char, close_length, rest)) = fence_delimiter(line)
                    && close_char == fence_char
                    && close_length >= length
                    && rest.trim().is_empty()
                {
                    open_fence = None;
                    finalizable = Some(offset);
                }
            }
        }
    }
    finalizable
}

fn fence_delimiter(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start();
    let fence_char = trimmed.chars().next().filter(|&c| c == '`' || c == '~')?;
    let length = trimmed.chars().take_while(|&c| c == fence_char).count();
    (length >= 3).then(|| (fence_char, length, &trimmed[length..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema as acp;

    fn running_tool_log(id: &str) -> ToolCallLog {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new(id.to_string(), "Running tool"));
        log
    }

    #[test]
    fn text_chunks_coalesce_into_trailing_segment() {
        let mut transcript = Transcript::new();
        transcript.append_text_chunk("Hel");
        transcript.append_text_chunk("lo");

        assert_eq!(transcript.pending(), [SegmentContent::Text("Hello".to_string())]);
    }

    #[test]
    fn text_chunks_keep_code_fences_in_one_segment() {
        let mut transcript = Transcript::new();
        transcript.append_text_chunk("Here you go:\n");
        transcript.append_text_chunk("```rust\n");
        transcript.append_text_chunk("fn main() {}\n");
        transcript.append_text_chunk("```\n");

        assert_eq!(
            transcript.pending(),
            [
                SegmentContent::Text("Here you go:\n".to_string()),
                SegmentContent::Text("```rust\nfn main() {}\n```\n".to_string()),
                SegmentContent::Text(String::new()),
            ]
        );
    }

    #[test]
    fn text_chunks_keep_embedded_fences_inside_longer_fences() {
        let mut transcript = Transcript::new();
        transcript.append_text_chunk("````markdown\n");
        transcript.append_text_chunk("```rust\n");
        transcript.append_text_chunk("````\n");

        assert_eq!(
            transcript.pending(),
            [SegmentContent::Text("````markdown\n```rust\n````\n".to_string()), SegmentContent::Text(String::new()),]
        );
    }

    #[test]
    fn thought_chunks_coalesce_until_block_closes() {
        let mut transcript = Transcript::new();
        transcript.append_thought_chunk("thinking");
        transcript.append_thought_chunk(" hard");
        transcript.close_thought_block();
        transcript.append_thought_chunk("again");

        assert_eq!(
            transcript.pending(),
            [SegmentContent::Thought("thinking hard".to_string()), SegmentContent::Thought("again".to_string())]
        );
    }

    #[test]
    fn text_chunk_closes_thought_block() {
        let mut transcript = Transcript::new();
        transcript.append_thought_chunk("thinking");
        transcript.append_text_chunk("answer");
        transcript.append_thought_chunk("more");

        assert_eq!(transcript.pending().len(), 3);
    }

    #[test]
    fn ensure_tool_segment_is_idempotent() {
        let mut transcript = Transcript::new();
        transcript.ensure_tool_segment("tool-1");
        transcript.ensure_tool_segment("tool-1");

        assert_eq!(transcript.pending(), [SegmentContent::ToolCall("tool-1".to_string())]);
    }

    #[test]
    fn drain_keeps_trailing_text_while_prompt_in_flight() {
        let mut transcript = Transcript::new();
        transcript.push_user_message("hi");
        transcript.append_text_chunk("streaming");

        let drained = transcript.drain_finalized_prefix(&ToolCallLog::new(), true);

        assert_eq!(drained, [SegmentContent::UserMessage("hi".to_string())]);
        assert_eq!(transcript.pending(), [SegmentContent::Text("streaming".to_string())]);
    }

    #[test]
    fn drain_takes_everything_after_prompt_completes() {
        let mut transcript = Transcript::new();
        transcript.push_user_message("hi");
        transcript.append_text_chunk("answer");

        let drained = transcript.drain_finalized_prefix(&ToolCallLog::new(), false);

        assert_eq!(drained.len(), 2);
        assert!(transcript.pending().is_empty());
    }

    #[test]
    fn drain_stops_at_running_tool_to_preserve_order() {
        let mut transcript = Transcript::new();
        transcript.push_user_message("hi");
        transcript.append_text_chunk("before");
        transcript.ensure_tool_segment("tool-1");
        transcript.append_text_chunk("after");

        let drained = transcript.drain_finalized_prefix(&running_tool_log("tool-1"), true);

        assert_eq!(
            drained,
            [SegmentContent::UserMessage("hi".to_string()), SegmentContent::Text("before".to_string())]
        );
        assert_eq!(
            transcript.pending(),
            [SegmentContent::ToolCall("tool-1".to_string()), SegmentContent::Text("after".to_string())]
        );
    }

    #[test]
    fn drain_takes_completed_tool() {
        let mut transcript = Transcript::new();
        transcript.ensure_tool_segment("tool-1");

        let drained = transcript.drain_finalized_prefix(&ToolCallLog::new(), true);

        assert_eq!(drained, [SegmentContent::ToolCall("tool-1".to_string())]);
    }
}
