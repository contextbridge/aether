use super::component::{Component, Event};
use crate::focus::{FocusOutcome, FocusRing};
use crate::rendering::frame::{Cursor, Frame, FramePart};
use crate::rendering::line::Line;
use crate::rendering::render_context::ViewContext;
use crate::style::Style;
use crossterm::event::KeyCode;

pub struct SplitWidths {
    pub left: u16,
    pub right: u16,
}

pub struct SplitLayout {
    base: SplitBase,
    step: i16,
    min_left: usize,
}

enum SplitBase {
    Fraction { numer: usize, denom: usize, min: usize, max: usize },
    Fixed(usize),
}

impl SplitLayout {
    pub fn fraction(numer: usize, denom: usize, min: usize, max: usize) -> Self {
        Self { base: SplitBase::Fraction { numer, denom, min, max }, step: 4, min_left: 12 }
    }

    pub fn fixed(width: usize) -> Self {
        Self { base: SplitBase::Fixed(width), step: 4, min_left: 12 }
    }

    pub fn with_step(mut self, step: i16) -> Self {
        self.step = step;
        self
    }

    pub fn with_min_left(mut self, min: usize) -> Self {
        self.min_left = min;
        self
    }

    fn widths(&self, total_width: u16, delta: i16) -> SplitWidths {
        let total = total_width as usize;
        let base = match self.base {
            SplitBase::Fraction { numer, denom, min, max } => (total * numer / denom).clamp(min, max),
            SplitBase::Fixed(w) => w,
        };
        let left = base.saturating_add_signed(delta.into()).clamp(self.min_left, total / 2);
        let right = total.saturating_sub(left + 1);
        SplitWidths { left: u16::try_from(left).unwrap_or(u16::MAX), right: u16::try_from(right).unwrap_or(u16::MAX) }
    }

    fn step(&self) -> i16 {
        self.step
    }
}

pub enum Either<L, R> {
    Left(L),
    Right(R),
}

pub struct SplitPanel<L: Component, R: Component> {
    left: L,
    right: R,
    layout: SplitLayout,
    delta: i16,
    focus: FocusRing,
    separator: Option<(String, Style)>,
    resize_keys: bool,
}

impl<L: Component, R: Component> SplitPanel<L, R> {
    pub fn new(left: L, right: R, layout: SplitLayout) -> Self {
        Self {
            left,
            right,
            layout,
            delta: 0,
            focus: FocusRing::new(2).without_wrap(),
            separator: None,
            resize_keys: false,
        }
    }

    pub fn with_separator(mut self, text: impl Into<String>, style: Style) -> Self {
        self.separator = Some((text.into(), style));
        self
    }

    pub fn with_resize_keys(mut self) -> Self {
        self.resize_keys = true;
        self
    }

    pub fn left(&self) -> &L {
        &self.left
    }

    pub fn left_mut(&mut self) -> &mut L {
        &mut self.left
    }

    pub fn right(&self) -> &R {
        &self.right
    }

    pub fn right_mut(&mut self) -> &mut R {
        &mut self.right
    }

    pub fn focus_left(&mut self) {
        self.focus.focus(0);
    }

    pub fn focus_right(&mut self) {
        self.focus.focus(1);
    }

    pub fn is_left_focused(&self) -> bool {
        self.focus.is_focused(0)
    }

    pub fn widths(&self, total_width: u16) -> SplitWidths {
        self.layout.widths(total_width, self.delta)
    }

    fn widen(&mut self) {
        self.delta += self.layout.step();
    }

    fn narrow(&mut self) {
        self.delta -= self.layout.step();
    }

    pub fn set_separator_style(&mut self, style: Style) {
        if let Some((_, s)) = &mut self.separator {
            *s = style;
        }
    }
}

impl<L: Component, R: Component> Component for SplitPanel<L, R> {
    type Message = Either<L::Message, R::Message>;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Key(key) = event {
            if self.resize_keys {
                match key.code {
                    KeyCode::Char('<') => {
                        self.narrow();
                        return Some(vec![]);
                    }
                    KeyCode::Char('>') => {
                        self.widen();
                        return Some(vec![]);
                    }
                    _ => {}
                }
            }

            let outcome = self.focus.handle_key(*key);
            if matches!(outcome, FocusOutcome::FocusChanged) {
                return Some(vec![]);
            }
        }

        if self.focus.is_focused(0) {
            self.left.on_event(event).await.map(|msgs| msgs.into_iter().map(Either::Left).collect())
        } else {
            self.right.on_event(event).await.map(|msgs| msgs.into_iter().map(Either::Right).collect())
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let widths = self.widths(ctx.size.width);
        let max_rows = ctx.size.height;
        let total_width = ctx.size.width;

        let mut left = self.left.render(&ctx.with_width(widths.left));
        let mut right = self.right.render(&ctx.with_width(widths.right));

        // Only the focused side may contribute the merged cursor — suppress the
        // other side's cursor before composition so hstack picks the right one.
        if !self.focus.is_focused(0) {
            left = left.with_cursor(Cursor::hidden());
        }
        if !self.focus.is_focused(1) {
            right = right.with_cursor(Cursor::hidden());
        }

        let left_part = FramePart::wrap(left, widths.left);
        let right_part = FramePart::wrap(right, widths.right);

        let merged = if let Some((text, style)) = &self.separator {
            let sep_proto = Line::with_style(text.clone(), *style);
            let sep_width = u16::try_from(sep_proto.display_width()).unwrap_or(0);
            let sep_rows = left_part.frame.lines().len().max(right_part.frame.lines().len()).max(usize::from(max_rows));
            let sep_lines: Vec<Line> = (0..sep_rows).map(|_| sep_proto.clone()).collect();
            Frame::hstack([left_part, FramePart::new(Frame::new(sep_lines), sep_width), right_part])
        } else {
            Frame::hstack([left_part, right_part])
        };

        merged.fit_height(max_rows, total_width)
    }
}
