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
}

impl<'a> ModalFrame<'a> {
    pub(crate) fn new(title: &'a str, footer: Option<Line<'a>>, width: Constraint, height: Constraint) -> Self {
        Self { title, footer, width, height }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Rect {
        let area = area.centered(self.width, self.height);
        Clear.render(area, buf);

        let mut block = Block::bordered()
            .title(format!(" {} ", self.title))
            .style(Style::new().bg(theme.background))
            .border_style(Style::new().fg(theme.accent));

        if let Some(mut footer) = self.footer.clone() {
            footer.spans.insert(0, Span::raw(" "));
            footer.spans.push(Span::raw(" "));
            block = block.title_bottom(footer);
        }
        let inner = block.inner(area);
        block.render(area, buf);
        inner
    }
}
