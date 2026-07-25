use crate::selection::SelectionState;
use crate::theme::Theme;
use crate::widgets::render_vertical_scrollbar;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, List, ListItem, Paragraph, StatefulWidget, Widget};

/// Rows drawn against a [`SelectionState`], with the chrome every list pane in
/// the UI puts around one: an optional border, an optional scrollbar, and a
/// placeholder for when there is nothing to show.
///
/// Rendering records where the rows landed, so a later click can be hit-tested
/// against the same area they were drawn into without re-deriving how many
/// borders and headers sit above the first row.
pub struct ListView<'a> {
    rows: Vec<ListItem<'static>>,
    selection: &'a mut SelectionState,
    theme: &'a Theme,
    empty_message: &'a str,
    block: Option<Block<'static>>,
    scrollbar: bool,
    highlight: Option<Style>,
    highlight_symbol: Option<&'static str>,
}

impl<'a> ListView<'a> {
    pub fn new(rows: Vec<ListItem<'static>>, selection: &'a mut SelectionState, theme: &'a Theme) -> Self {
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
        self.block(Block::bordered().title(title.into()).style(style)).scrollbar()
    }

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

        let row_count = rows.len();
        let mut list = List::new(rows)
            .highlight_style(highlight.unwrap_or_else(|| Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)))
            .scroll_padding(1);
        if let Some(symbol) = highlight_symbol {
            list = list.highlight_symbol(symbol);
        }
        selection.set_rows_area(inner);
        StatefulWidget::render(list, inner, buf, selection.list_state_mut());

        if scrollbar {
            render_vertical_scrollbar(inner, buf, row_count, selection.offset());
        }
    }
}
