use super::{Composer, ComposerLayout};
use crate::attachments::{AttachmentKind, classify_attachment};
use crate::theme::Theme;
use crate::wrap::{fit_prefix, wrap_text_char};
use ratatui::layout::Position;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

/// Columns the `> ` / `  ` line prefix occupies.
const PREFIX_WIDTH: u16 = 2;

/// The composer's input as it is drawn: one byte range per visual row, and
/// where the cursor sits among them.
///
/// Vertical cursor movement and rendering both read this, so a keystroke can
/// never disagree with the rows the user is looking at.
pub(super) struct InputLayout {
    pub rows: Vec<Range<usize>>,
    pub cursor_row: usize,
    pub cursor_column: usize,
}

/// Lays `text` out over `content_width` columns.
///
/// A cursor at the end of a completely full row has no column left to occupy
/// there, so it moves to the start of the row below — one that is added empty
/// when the wrap does not already continue onto it.
pub(super) fn input_layout(text: &str, cursor_byte: usize, content_width: usize) -> InputLayout {
    let mut rows = wrapped_rows(text, content_width);
    let index = rows.iter().position(|row| cursor_byte <= row.end).unwrap_or(rows.len().saturating_sub(1));
    let column = text[rows[index].start..cursor_byte].width();
    if column < content_width {
        return InputLayout { rows, cursor_row: index, cursor_column: column };
    }
    if rows.get(index + 1).is_none_or(|next| next.start != rows[index].end) {
        rows.insert(index + 1, cursor_byte..cursor_byte);
    }
    InputLayout { rows, cursor_row: index + 1, cursor_column: 0 }
}

impl InputLayout {
    /// The byte offset `column` maps to on `row`, clamped to the row's end when
    /// the row is too short to reach it.
    pub(super) fn byte_at(&self, text: &str, row: usize, column: usize) -> Option<usize> {
        let row = self.rows.get(row)?;
        Some(row.start + fit_prefix(&text[row.clone()], column).0)
    }
}

impl Composer {
    /// Lays the composer out for `width` columns: a rule, the wrapped input with
    /// its `@mention`s highlighted, any attachments, and a closing rule.
    ///
    /// The width is remembered so vertical cursor movement follows the same rows
    /// this produced.
    pub fn layout(&mut self, width: u16, theme: &Theme) -> ComposerLayout {
        let content_width = usize::from(width.saturating_sub(PREFIX_WIDTH).max(1));
        self.set_content_width(content_width);
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
        let mentions = mention_byte_ranges(text);
        let layout = input_layout(text, self.cursor_byte(), content_width);
        let first_row = u16::try_from(lines.len()).unwrap_or(u16::MAX);

        for (index, row) in layout.rows.iter().enumerate() {
            let prefix = if index == 0 { "> " } else { "  " };
            let mut spans = vec![Span::styled(prefix, Style::new().fg(theme.accent))];
            spans.extend(styled_input_chunk(&text[row.clone()], row.start, &mentions, theme));
            lines.push(Line::from(spans));
        }

        Position::new(
            u16::try_from(layout.cursor_column).unwrap_or(u16::MAX).saturating_add(PREFIX_WIDTH),
            first_row.saturating_add(u16::try_from(layout.cursor_row).unwrap_or(u16::MAX)),
        )
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

/// The byte range each visual row of `text` shows once it is hard-wrapped at
/// `content_width`. Always at least one row, so an empty composer still has a
/// row for its cursor.
fn wrapped_rows(text: &str, content_width: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        for chunk in wrap_text_char(line, content_width) {
            rows.push(offset..offset + chunk.len());
            offset += chunk.len();
        }
        // Step over the newline the split consumed.
        offset += 1;
    }
    rows
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
