use pulldown_cmark::Alignment;
use unicode_width::UnicodeWidthStr;

use crate::line::Line;
use crate::span::Span;
use crate::style::Style;
use crate::theme::Theme;

/// A single rendered cell in a table row.
#[derive(Clone, Debug)]
pub(crate) struct TableCell {
    /// Styled content lines.
    pub(crate) lines: Vec<Line>,
    /// Horizontal alignment for this cell.
    pub(crate) alignment: Alignment,
    /// Maximum display width across `lines`.
    pub(crate) max_width: usize,
}

impl Default for TableCell {
    fn default() -> Self {
        Self { lines: Vec::new(), alignment: Alignment::None, max_width: 0 }
    }
}

/// A row in a table.
pub(crate) type TableRow = Vec<TableCell>;

/// Builds styled inline content for a single table cell.
#[derive(Clone, Debug, Default)]
pub(crate) struct CellBuilder {
    lines: Vec<Line>,
    current_line: Line,
}

impl CellBuilder {
    pub(crate) fn push_text(&mut self, text: &str, style: Style) {
        for (i, chunk) in text.split('\n').enumerate() {
            if i > 0 {
                self.flush_line();
            }
            if !chunk.is_empty() {
                self.current_line.push_span(Span::with_style(chunk, style));
            }
        }
    }

    pub(crate) fn push_code(&mut self, code: &str, style: Style) {
        if !code.is_empty() {
            self.current_line.push_span(Span::with_style(code, style));
        }
    }

    pub(crate) fn soft_break(&mut self, style: Style) {
        self.current_line.push_span(Span::with_style(" ", style));
    }

    pub(crate) fn hard_break(&mut self) {
        self.flush_line();
    }

    pub(crate) fn finish(mut self) -> Vec<Line> {
        if !self.current_line.is_empty() || !self.lines.is_empty() {
            self.lines.push(std::mem::take(&mut self.current_line));
        }
        self.lines
    }

    fn flush_line(&mut self) {
        let line = std::mem::take(&mut self.current_line);
        self.lines.push(line);
    }
}

/// Manages table state during parsing and rendering.
#[derive(Clone, Debug, Default)]
pub(crate) struct TableState {
    /// Column alignments from markdown table syntax.
    pub(crate) alignments: Vec<Alignment>,
    /// All rows in the table (including header).
    rows: Vec<TableRow>,
    /// Current row being built.
    pub(crate) current_row: Vec<TableCell>,
    /// Display width for each column, including left/right padding.
    column_widths: Vec<usize>,
}

impl TableState {
    pub(crate) fn new(alignments: &[Alignment]) -> Self {
        Self {
            alignments: alignments.to_vec(),
            rows: Vec::new(),
            current_row: Vec::new(),
            column_widths: vec![0; alignments.len()],
        }
    }

    pub(crate) fn start_row(&mut self) {
        self.current_row.clear();
    }

    pub(crate) fn add_cell(&mut self, cell: TableCell) {
        let col_idx = self.current_row.len();
        let needed = cell.max_width + 2;
        if col_idx < self.column_widths.len() {
            self.column_widths[col_idx] = self.column_widths[col_idx].max(needed);
        }
        self.current_row.push(cell);
    }

    pub(crate) fn finish_row(&mut self) {
        if !self.current_row.is_empty() {
            self.rows.push(std::mem::take(&mut self.current_row));
        }
    }

    fn cell_width(&self, col_idx: usize) -> usize {
        self.column_widths.get(col_idx).copied().unwrap_or(0).max(3)
    }

    pub(crate) fn render(&self, theme: &Theme) -> Vec<Line> {
        if self.rows.is_empty() {
            return Vec::new();
        }

        let num_cols = self.column_widths.len();
        if num_cols == 0 {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let border_style = Style::fg(theme.muted());
        lines.push(self.render_border(num_cols, '┌', '┬', '┐', border_style));

        for (row_idx, row) in self.rows.iter().enumerate() {
            let max_cell_lines = (0..num_cols)
                .map(|col_idx| row.get(col_idx).map_or(1, |cell| cell.lines.len().max(1)))
                .max()
                .unwrap_or(1);

            for line_idx in 0..max_cell_lines {
                let mut line = Line::default();
                line.push_span(Span::with_style("│", border_style));

                for col_idx in 0..num_cols {
                    let width = self.cell_width(col_idx);
                    let cell = row.get(col_idx);
                    let alignment = cell.map_or_else(|| self.alignments[col_idx], |c| c.alignment);
                    let content_line = cell.and_then(|c| c.lines.get(line_idx));
                    Self::push_formatted_cell_line(&mut line, content_line, width, alignment);

                    if col_idx < num_cols - 1 {
                        line.push_span(Span::with_style("│", border_style));
                    }
                }

                line.push_span(Span::with_style("│", border_style));
                lines.push(line);
            }

            if row_idx == 0 {
                lines.push(self.render_border(num_cols, '├', '┼', '┤', border_style));
            }
        }

        lines.push(self.render_border(num_cols, '└', '┴', '┘', border_style));
        lines
    }

    fn render_border(&self, num_cols: usize, left_char: char, mid_char: char, right_char: char, style: Style) -> Line {
        let mut s = String::new();
        s.push(left_char);
        for col_idx in 0..num_cols {
            for _ in 0..self.cell_width(col_idx) {
                s.push('─');
            }
            if col_idx < num_cols - 1 {
                s.push(mid_char);
            }
        }
        s.push(right_char);
        Line::with_style(s, style)
    }

    fn push_formatted_cell_line(line: &mut Line, content_line: Option<&Line>, width: usize, alignment: Alignment) {
        let cell_width = width.max(3);
        let content_width = content_line.map_or(0, line_display_width);
        let padding = cell_width.saturating_sub(content_width);

        let (left_pad, right_pad) = match alignment {
            Alignment::Right => (padding.saturating_sub(1), 1),
            Alignment::Center => {
                let left = padding / 2;
                let right = padding.saturating_sub(left);
                (left, right)
            }
            _ => (1, padding.saturating_sub(1)),
        };

        if left_pad > 0 {
            line.push_span(Span::with_style(" ".repeat(left_pad), Style::default()));
        }
        if let Some(content) = content_line {
            line.append_line(content);
        }
        if right_pad > 0 {
            line.push_span(Span::with_style(" ".repeat(right_pad), Style::default()));
        }
    }
}

pub(super) fn line_display_width(line: &Line) -> usize {
    line.spans().iter().map(|span| UnicodeWidthStr::width(span.text())).sum()
}
