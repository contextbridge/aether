use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Widget};

use crate::components::theme::Theme;

pub(crate) struct ModalFrame<'a> {
    title: &'a str,
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
        Self { title, footer, width, height, theme }
    }

    pub(crate) fn area(&self, outer: Rect) -> Rect {
        outer.centered(self.width, self.height)
    }

    pub(crate) fn inner(&self, outer: Rect) -> Rect {
        let area = self.area(outer);
        self.block().inner(area)
    }

    fn block(&self) -> Block<'_> {
        let mut block = Block::bordered()
            .title(format!(" {} ", self.title))
            .style(Style::new().bg(self.theme.background))
            .border_style(Style::new().fg(self.theme.accent));

        if let Some(mut footer) = self.footer.clone() {
            footer.spans.insert(0, Span::raw(" "));
            footer.spans.push(Span::raw(" "));
            block = block.title_bottom(footer);
        }
        block
    }
}

impl Widget for &ModalFrame<'_> {
    fn render(self, outer: Rect, buf: &mut Buffer) {
        let area = self.area(outer);
        Clear.render(area, buf);
        self.block().render(area, buf);
    }
}
