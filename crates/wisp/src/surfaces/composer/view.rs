use super::{Composer, ComposerLayout};
use crate::attachment::{AttachmentKind, classify_attachment};
use crate::theme::Theme;
use crate::view::widgets::RowsView;
use crate::view::wrap::{fit_prefix, wrap_text_char};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
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
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        for chunk in wrap_text_char(line, content_width) {
            rows.push(offset..offset + chunk.len());
            offset += chunk.len();
        }
        offset += 1;
    }
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

/// The composer body as a clipped, one-frame widget. Its cursor calculation
/// uses the same visible range as its painting, so terminal placement cannot
/// drift from wrapped input rows.
pub(crate) struct ComposerBodyView<'a> {
    layout: &'a ComposerLayout,
    first_row: usize,
}

impl<'a> ComposerBodyView<'a> {
    pub(crate) fn new(layout: &'a ComposerLayout, first_row: usize) -> Self {
        Self { layout, first_row }
    }

    pub(crate) fn cursor_position(&self, area: Rect) -> Option<Position> {
        let cursor = self.layout.cursor;
        if usize::from(cursor.y) < self.first_row {
            return None;
        }
        let x = area.x.saturating_add(cursor.x);
        let y = area.y.saturating_add(u16::try_from(usize::from(cursor.y) - self.first_row).unwrap_or(u16::MAX));
        (x < area.right() && y < area.bottom()).then_some(Position::new(x, y))
    }
}

impl Widget for ComposerBodyView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        RowsView::new(&self.layout.lines[self.first_row.min(self.layout.lines.len())..]).render(area, buf);
    }
}

impl Composer {
    /// Adopts a new terminal width, so wrapping and vertical cursor movement
    /// follow the rows the next layout will produce.
    pub fn on_resize(&mut self, width: u16) {
        self.content_width = Some(usize::from(width.saturating_sub(PREFIX_WIDTH).max(1)));
    }

    /// Lays the composer out for `width` columns: a rule, the wrapped input with
    /// its `@mention`s highlighted, any attachments, and a closing rule.
    pub fn layout(&self, width: u16, theme: &Theme) -> ComposerLayout {
        let content_width = usize::from(width.saturating_sub(PREFIX_WIDTH).max(1));
        let rule = Line::styled("─".repeat(usize::from(width)), Style::new().fg(theme.muted));

        let mut lines = vec![rule.clone()];
        let cursor = self.push_input_lines(&mut lines, content_width, theme);
        lines.extend(self.pending_media().iter().map(|attachment| {
            let label = match classify_attachment(&attachment.path) {
                AttachmentKind::Image => "image",
                AttachmentKind::Audio => "audio",
                _ => "file",
            };
            Line::styled(format!("  attached {label}: {}", attachment.display_name), Style::new().fg(theme.info))
        }));
        lines.push(rule);

        ComposerLayout { lines, cursor }
    }

    /// Appends the wrapped input rows, returning where the cursor landed.
    fn push_input_lines(&self, lines: &mut Vec<Line<'static>>, content_width: usize, theme: &Theme) -> Position {
        let text = self.text();
        let mentions: Vec<Range<usize>> = text
            .match_indices('@')
            .filter_map(|(at_pos, _)| {
                let end = text[at_pos..].find(char::is_whitespace).map_or(text.len(), |offset| at_pos + offset);
                (end > at_pos).then_some(at_pos..end)
            })
            .collect();
        let layout = input_layout(text, self.buffer.cursor(), content_width);
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
