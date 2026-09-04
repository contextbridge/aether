use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Widget};

use crate::theme::Theme;

pub(crate) const MODAL_HORIZONTAL_PADDING: u16 = 2;
pub(crate) const MODAL_VERTICAL_PADDING: u16 = 1;

/// The header and footer take the same padding as the content on every side,
/// so all three sit equally far inside the modal instead of the title and key
/// hints hugging the frame's edges.
const HEADER_FOOTER_VERTICAL_PADDING: u16 = MODAL_VERTICAL_PADDING;

/// Rows the frame takes besides its content: the header and footer rows, the
/// blank rows padding them vertically, and the content's own vertical padding.
pub(crate) const MODAL_VERTICAL_CHROME: u16 = 2 + 2 * HEADER_FOOTER_VERTICAL_PADDING + 2 * MODAL_VERTICAL_PADDING;

pub(crate) struct ModalFrame<'a> {
    title: &'a str,
    title_right: Option<&'a str>,
    footer: Option<Line<'a>>,
    width: Constraint,
    height: Constraint,
    theme: &'a Theme,
}

impl<'a> ModalFrame<'a> {
    pub(crate) fn new(
        title: &'a str,
        footer: Option<Line<'a>>,
        width: Constraint,
        height: Constraint,
        theme: &'a Theme,
    ) -> Self {
        Self { title, title_right: None, footer, width, height, theme }
    }

    /// A small right-aligned chip naming who is asking, so the request reads
    /// `Request ─ server` instead of protocol vocabulary.
    pub(crate) fn title_right(mut self, name: &'a str) -> Self {
        self.title_right = Some(name);
        self
    }

    pub(crate) fn area(&self, outer: Rect) -> Rect {
        outer.centered(self.width, self.height)
    }

    pub(crate) fn inner(&self, outer: Rect) -> Rect {
        self.block().inner(chrome_area(self.area(outer)))
    }

    fn block(&self) -> Block<'_> {
        let horizontal_padding = " ".repeat(usize::from(MODAL_HORIZONTAL_PADDING));
        let mut block = Block::default()
            .title_top(Span::styled(
                format!("{horizontal_padding}{}{horizontal_padding}", self.title),
                Style::new().fg(self.theme.accent).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::new(
                MODAL_HORIZONTAL_PADDING,
                MODAL_HORIZONTAL_PADDING,
                MODAL_VERTICAL_PADDING,
                MODAL_VERTICAL_PADDING,
            ))
            .style(Style::new().bg(self.theme.background));

        if let Some(name) = self.title_right {
            block = block.title_top(
                Line::from(Span::styled(
                    format!("{horizontal_padding}{name}{horizontal_padding}"),
                    Style::new().fg(self.theme.muted),
                ))
                .right_aligned(),
            );
        }

        if let Some(mut footer) = self.footer.clone() {
            footer.spans.insert(0, Span::raw(horizontal_padding.clone()));
            footer.spans.push(Span::raw(horizontal_padding));
            block = block.title_bottom(footer);
        }
        block
    }
}

impl Widget for &ModalFrame<'_> {
    fn render(self, outer: Rect, buf: &mut Buffer) {
        let area = self.area(outer);
        Clear.render(area, buf);
        // Clear leaves the terminal's own background; the modal paints its own
        // over every row, including the blank rows above the header and below
        // the footer that the chrome skips.
        buf.set_style(area, Style::new().bg(self.theme.background));
        self.block().render(chrome_area(area), buf);
    }
}

/// The modal with the header and footer inset by their vertical padding. A
/// modal too short to keep a content row past the inset keeps its chrome on
/// the edges instead, so a tiny terminal still shows content.
fn chrome_area(area: Rect) -> Rect {
    let padding = HEADER_FOOTER_VERTICAL_PADDING;
    let rows_around_content = 2 * padding + 2 + 2 * MODAL_VERTICAL_PADDING;
    if area.height < rows_around_content + 1 {
        return area;
    }
    Rect { y: area.y.saturating_add(padding), height: area.height - 2 * padding, ..area }
}
