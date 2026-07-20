use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Widget};

use crate::theme::Theme;

pub(crate) const MODAL_HORIZONTAL_PADDING: u16 = 2;
pub(crate) const MODAL_VERTICAL_PADDING: u16 = 1;

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
        let area = self.area(outer);
        self.block().inner(area)
    }

    fn block(&self) -> Block<'_> {
        let horizontal_padding = " ".repeat(usize::from(MODAL_HORIZONTAL_PADDING));
        let mut block = Block::default()
            .title_top(Span::styled(
                format!("{horizontal_padding}{}{horizontal_padding}", self.title),
                Style::new().fg(self.theme.accent).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::proportional(1))
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
        self.block().render(area, buf);
    }
}
