use crate::components::selection::{SelectionState, scroll_into_view};
use crate::components::theme::Theme;
use crate::components::widgets::{render_vertical_scrollbar, row_area, rows_and_track};
use crate::components::wrap::{as_u16, fit_line};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr;

/// Rows drawn against a [`SelectionState`], with the chrome every list pane in
/// the UI puts around one: an optional border, an optional scrollbar, and a
/// placeholder for when there is nothing to show.
///
/// Only the rows on screen are built, so the pickers that index a whole working
/// tree cost a screenful of work per frame rather than one row per entry.
///
/// Rows are fitted to the columns actually left over here, so callers building
/// them never work out how much the border, highlight symbol, and scrollbar
/// take. Rendering records where the rows landed, so a later click can be
/// hit-tested against the same area they were drawn into.
pub struct ListView<'a> {
    rows: Rows<'a>,
    theme: &'a Theme,
    empty_message: &'a str,
    block: Option<Block<'static>>,
    scrollbar: bool,
    highlight: Option<Style>,
    highlight_symbol: Option<&'static str>,
}

impl<'a> ListView<'a> {
    /// Rows already in hand, for the lists short enough that building them all
    /// costs nothing.
    pub fn new(rows: Vec<Line<'static>>, theme: &'a Theme) -> Self {
        let len = rows.len();
        let mut rows = rows;
        Self::lazy(len, move |index| std::mem::take(&mut rows[index]), theme)
    }

    /// `len` rows, each built by `row` only if it is drawn.
    pub fn lazy(len: usize, row: impl FnMut(usize) -> Line<'static> + 'a, theme: &'a Theme) -> Self {
        Self {
            rows: Rows { len, build: Box::new(row) },
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

    /// The chrome every settings pane shares: an inverted highlight and the
    /// standard "nothing here" placeholder.
    pub fn pane(self, empty_message: &'a str) -> Self {
        let highlight = Style::new().fg(self.theme.background).bg(self.theme.text_primary);
        self.empty_message(empty_message).highlight_style(highlight)
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

impl StatefulWidget for ListView<'_> {
    type State = SelectionState;

    fn render(self, area: Rect, buf: &mut Buffer, selection: &mut Self::State) {
        let Self { mut rows, theme, empty_message, block, scrollbar, highlight, highlight_symbol } = self;
        let inner = block.as_ref().map_or(area, |block| block.inner(area));
        if let Some(block) = block {
            block.render(area, buf);
        }

        if rows.len == 0 {
            selection.set_rows_area(Rect::ZERO);
            Paragraph::new(empty_message).style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let (rows_area, track_area) = rows_and_track(inner, scrollbar);
        selection.set_rows_area(rows_area);
        let height = usize::from(rows_area.height);
        if height == 0 {
            return;
        }

        let selected = selection.selected().map(|selected| selected.min(rows.len - 1));
        let offset = visible_offset(selection.offset(), selected, rows.len, height);
        // Clicks are hit-tested against the offset the rows were drawn from, so
        // the window this frame settled on has to be written back.
        selection.set_offset(offset);

        let symbol_width = as_u16(highlight_symbol.map_or(0, str::width));
        let content_width = usize::from(rows_area.width.saturating_sub(symbol_width));
        let highlight = highlight.unwrap_or_else(|| Style::new().fg(theme.text_primary).bg(theme.sidebar_bg));

        for (drawn, index) in (offset..rows.len.min(offset + height)).enumerate() {
            let Some(whole) = row_area(rows_area, drawn) else {
                break;
            };
            let content = Rect { x: whole.x + symbol_width, width: whole.width.saturating_sub(symbol_width), ..whole };
            fit_line(rows.build(index), content_width, Style::default()).render(content, buf);
            if selected == Some(index) {
                // Painted over the row rather than patched into its spans, so a
                // highlight always wins against whatever colours the row uses.
                buf.set_style(whole, highlight);
                if let Some(symbol) = highlight_symbol {
                    Line::raw(symbol).render(Rect { width: symbol_width, ..whole }, buf);
                }
            }
        }

        if scrollbar {
            render_vertical_scrollbar(track_area, buf, rows.len, offset);
        }
    }
}

/// A list's rows, built by index on demand.
struct Rows<'a> {
    len: usize,
    build: Box<dyn FnMut(usize) -> Line<'static> + 'a>,
}

impl Rows<'_> {
    fn build(&mut self, index: usize) -> Line<'static> {
        (self.build)(index)
    }
}

/// The first row to draw: `offset` moved the least it can to keep `selected` on
/// screen, with a row of context beyond it where the viewport allows.
///
/// This is the scrolling [`List`](ratatui::widgets::List) does from its own
/// `ListState`, reproduced for one-row items because choosing the window up
/// front is what lets the rest of the rows go unbuilt.
fn visible_offset(offset: usize, selected: Option<usize>, len: usize, height: usize) -> usize {
    let last = len.saturating_sub(1);
    let offset = offset.min(last);
    let Some(selected) = selected else {
        return offset;
    };
    // The padding is dropped rather than honoured on a viewport too short to
    // show the selection with a row either side of it.
    let padding = usize::from(height >= 3);
    let target = if (selected + padding).min(last) >= offset + height {
        (selected + padding).min(last)
    } else if selected.saturating_sub(padding) < offset {
        selected.saturating_sub(padding)
    } else {
        selected
    };
    scroll_into_view(offset, target, height)
}

#[cfg(test)]
mod tests {
    use super::{ListView, visible_offset};
    use crate::components::selection::SelectionState;
    use crate::components::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Line;

    #[test]
    fn builds_only_the_rows_it_draws() {
        let theme = Theme::default();
        let mut selection = SelectionState::new(50_000);
        selection.select(Some(1_000), 50_000);
        let mut built: Vec<usize> = Vec::new();

        let mut terminal = Terminal::new(TestBackend::new(8, 3)).unwrap();
        terminal
            .draw(|frame| {
                let rows = |index: usize| {
                    built.push(index);
                    Line::raw(index.to_string())
                };
                frame.render_stateful_widget(ListView::lazy(50_000, rows, &theme), frame.area(), &mut selection);
            })
            .unwrap();

        assert_eq!(built, vec![999, 1_000, 1_001], "only the visible window is formatted");
    }

    #[test]
    fn keeps_a_row_of_context_beyond_the_selection() {
        assert_eq!(visible_offset(0, Some(4), 20, 5), 1, "scrolls one past the selection moving down");
        assert_eq!(visible_offset(5, Some(5), 20, 5), 4, "scrolls one before the selection moving up");
        assert_eq!(visible_offset(0, Some(2), 20, 5), 0, "a selection with context either side stays put");
    }

    #[test]
    fn drops_the_context_row_when_the_viewport_cannot_hold_it() {
        assert_eq!(visible_offset(0, Some(1), 20, 2), 0, "the selection is already visible");
        assert_eq!(visible_offset(0, Some(2), 20, 2), 1, "scrolls only as far as the selection");
    }

    #[test]
    fn clamps_to_the_rows_that_exist() {
        assert_eq!(visible_offset(0, Some(19), 20, 5), 15, "the last row cannot scroll past the end");
        assert_eq!(visible_offset(30, Some(19), 20, 5), 18, "an offset past the end is pulled back to the rows");
        assert_eq!(visible_offset(3, None, 20, 5), 3, "an unselected list keeps its offset");
    }
}
