use crate::selection::SelectionState;
use crate::theme::Theme;
use crate::widgets::render_vertical_scrollbar;
use crate::wrap::fit_line;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr;

/// Rows drawn against a [`SelectionState`], with the chrome every list pane in
/// the UI puts around one: an optional border, an optional scrollbar, and a
/// placeholder for when there is nothing to show.
///
/// Rows are fitted to the columns actually left over here, so callers building
/// them never work out how much the border, highlight symbol, and scrollbar
/// take. Rendering records where the rows landed, so a later click can be
/// hit-tested against the same area they were drawn into.
pub struct ListView<'a> {
    rows: Vec<Line<'static>>,
    selection: &'a mut SelectionState,
    theme: &'a Theme,
    empty_message: &'a str,
    block: Option<Block<'static>>,
    scrollbar: bool,
    highlight: Option<Style>,
    highlight_symbol: Option<&'static str>,
}

impl<'a> ListView<'a> {
    pub fn new(rows: Vec<Line<'static>>, selection: &'a mut SelectionState, theme: &'a Theme) -> Self {
        Self {
            rows,
            selection,
            theme,
            empty_message: "",
            block: None,
            scrollbar: false,
            highlight: None,
            highlight_symbol: None,
        }
    }

    /// Shown in place of the rows when there are none.
    pub fn empty_message(mut self, message: &'a str) -> Self {
        self.empty_message = message;
        self
    }

    pub fn block(mut self, block: Block<'static>) -> Self {
        self.block = Some(block);
        self
    }

    /// Wraps the list in a titled border, the standard full-pane picker chrome.
    pub fn bordered(self, title: impl Into<String>) -> Self {
        let style = Style::new().fg(self.theme.text_primary);
        self.block(Block::bordered().title(title.into()).style(style))
    }

    /// Reserves the rightmost column for a scrollbar, so the track never sits on
    /// top of the rows.
    pub fn scrollbar(mut self) -> Self {
        self.scrollbar = true;
        self
    }

    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight = Some(style);
        self
    }

    pub fn highlight_symbol(mut self, symbol: &'static str) -> Self {
        self.highlight_symbol = Some(symbol);
        self
    }
}

impl Widget for ListView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Self { rows, selection, theme, empty_message, block, scrollbar, highlight, highlight_symbol } = self;
        let inner = block.as_ref().map_or(area, |block| block.inner(area));
        if let Some(block) = block {
            block.render(area, buf);
        }

        if rows.is_empty() {
            selection.set_rows_area(Rect::ZERO);
            Paragraph::new(empty_message).style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let track_width = u16::from(scrollbar);
        let [rows_area, track_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(track_width)]).areas(inner);
        let content_width = usize::from(rows_area.width).saturating_sub(highlight_symbol.map_or(0, str::width));

        let row_count = rows.len();
        let items: Vec<ListItem<'static>> =
            rows.into_iter().map(|row| ListItem::new(fit_line(row, content_width, Style::default()))).collect();
        let mut list = List::new(items)
            .highlight_style(highlight.unwrap_or_else(|| Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)))
            .scroll_padding(1);
        if let Some(symbol) = highlight_symbol {
            list = list.highlight_symbol(symbol);
        }
        selection.set_rows_area(rows_area);
        StatefulWidget::render(list, rows_area, buf, selection.list_state_mut());

        if scrollbar {
            render_vertical_scrollbar(track_area, buf, row_count, selection.offset());
        }
    }
}
