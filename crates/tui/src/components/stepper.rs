use crate::rendering::line::Line;
use crate::rendering::render_context::ViewContext;
use crate::rendering::span::Span;
use crate::rendering::style::Style;

pub enum StepVisualState {
    Complete,
    Current,
    Upcoming,
}

pub struct StepperItem<'a> {
    pub label: &'a str,
    pub state: StepVisualState,
}

pub struct Stepper<'a> {
    pub items: &'a [StepperItem<'a>],
    pub separator: &'a str,
    pub leading_padding: usize,
}

impl Stepper<'_> {
    pub fn render(&self, ctx: &ViewContext) -> Line {
        let padding = " ".repeat(self.leading_padding);
        let mut line = Line::new(padding);
        for (i, item) in self.items.iter().enumerate() {
            let (glyph, style) = match item.state {
                StepVisualState::Complete => ("\u{25cf} ", Style::fg(ctx.theme.text_secondary())),
                StepVisualState::Current => ("\u{25c9} ", Style::fg(ctx.theme.primary())),
                StepVisualState::Upcoming => ("\u{25cb} ", Style::fg(ctx.theme.muted())),
            };
            line.push_span(Span::with_style(glyph.to_string(), style));
            line.push_span(Span::with_style(item.label.to_string(), style));
            if i + 1 < self.items.len() {
                line.push_span(Span::with_style(self.separator.to_string(), Style::fg(ctx.theme.muted())));
            }
        }
        line
    }
}
