use super::{Composer, ComposerLayout};
use crate::attachments::{AttachmentKind, classify_attachment};
use crate::theme::Theme;
use crate::wrap::wrap_text_char;
use ratatui::layout::Position;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Columns the `> ` / `  ` line prefix occupies.
const PREFIX_WIDTH: u16 = 2;

impl Composer {
    /// Lays the composer out for `width` columns: a rule, the wrapped input with
    /// its `@mention`s highlighted, any attachments, and a closing rule.
    pub fn layout(&self, width: u16, theme: &Theme) -> ComposerLayout {
        let content_width = usize::from(width.saturating_sub(PREFIX_WIDTH).max(1));
        let rule = Line::styled("─".repeat(usize::from(width)), Style::new().fg(theme.muted));

        let mut lines = vec![rule.clone()];
        let cursor = self.push_input_lines(&mut lines, content_width, theme);
        lines.extend(self.attachment_lines(theme));
        lines.push(rule);

        ComposerLayout { lines, cursor }
    }

    /// Appends the wrapped input rows, returning where the cursor landed.
    fn push_input_lines(&self, lines: &mut Vec<Line<'static>>, content_width: usize, theme: &Theme) -> Position {
        let text = self.text();
        let cursor_byte = self.cursor_byte();
        let mentions = mention_byte_ranges(text);
        let mut cursor = Position::new(PREFIX_WIDTH, 0);
        let mut byte_offset = 0;

        for (logical_row, raw_line) in text.split('\n').enumerate() {
            let prefix = if logical_row == 0 { "> " } else { "  " };
            for (wrapped_row, chunk) in wrap_text_char(raw_line, content_width).iter().enumerate() {
                let row = u16::try_from(lines.len()).unwrap_or(u16::MAX);
                let mut spans =
                    vec![Span::styled(if wrapped_row == 0 { prefix } else { "  " }, Style::new().fg(theme.accent))];
                spans.extend(styled_input_chunk(chunk, byte_offset, &mentions, theme));
                lines.push(Line::from(spans));

                let chunk_end = byte_offset + chunk.len();
                // The trailing chunk owns a cursor sitting at the very end of
                // the text, which is one past its last byte.
                if cursor_byte >= byte_offset && (cursor_byte < chunk_end || chunk_end == text.len()) {
                    let column = text[byte_offset..cursor_byte].width();
                    cursor = Position::new(u16::try_from(column).unwrap_or(u16::MAX).saturating_add(PREFIX_WIDTH), row);
                }
                byte_offset = chunk_end;
            }
            // Step over the newline the split consumed.
            if logical_row + 1 < self.line_count() {
                byte_offset += 1;
            }
        }
        cursor
    }

    fn attachment_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        self.pending_media()
            .iter()
            .map(|attachment| {
                let label = match classify_attachment(&attachment.path) {
                    AttachmentKind::Image => "image",
                    AttachmentKind::Audio => "audio",
                    _ => "file",
                };
                Line::styled(format!("  attached {label}: {}", attachment.display_name), Style::new().fg(theme.info))
            })
            .collect()
    }
}

/// Byte ranges of `@mention` tokens (an `@` followed by non-whitespace) within the whole
/// composer text. Newlines count as whitespace, so mentions never span lines.
fn mention_byte_ranges(input: &str) -> Vec<std::ops::Range<usize>> {
    input
        .match_indices('@')
        .filter_map(|(at_pos, _)| {
            let end = input[at_pos..].find(char::is_whitespace).map_or(input.len(), |offset| at_pos + offset);
            (end > at_pos).then_some(at_pos..end)
        })
        .collect()
}

/// Split a wrapped input chunk into styled spans, colouring any `@mention` bytes that overlap
/// the chunk (identified by their absolute byte offset in the full composer text) in the info
/// colour and everything else in the primary text colour.
fn styled_input_chunk(
    chunk: &str,
    chunk_start: usize,
    mention_ranges: &[std::ops::Range<usize>],
    theme: &Theme,
) -> Vec<Span<'static>> {
    let in_mention_at = |relative: usize| mention_ranges.iter().any(|range| range.contains(&(chunk_start + relative)));

    if mention_ranges.is_empty() || !chunk.contains('@') {
        return vec![Span::styled(chunk.to_string(), Style::new().fg(theme.text_primary))];
    }

    let mut spans = Vec::new();
    let mut run_start = 0;
    let mut current_info = in_mention_at(0);
    for (relative, _) in chunk.char_indices().skip(1) {
        let is_info = in_mention_at(relative);
        if is_info != current_info {
            let color = if current_info { theme.info } else { theme.text_primary };
            spans.push(Span::styled(chunk[run_start..relative].to_string(), Style::new().fg(color)));
            run_start = relative;
            current_info = is_info;
        }
    }
    let color = if current_info { theme.info } else { theme.text_primary };
    spans.push(Span::styled(chunk[run_start..].to_string(), Style::new().fg(color)));
    spans
}
