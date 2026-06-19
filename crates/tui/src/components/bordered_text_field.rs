use crate::components::text_field::TextField;
use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::rendering::soft_wrap::{display_width_text, truncate_line};
use crate::style::Style;

/// Fixed columns consumed by the two side borders: `│` + space on each side.
const HORIZONTAL_PADDING: usize = 4;
/// Fixed columns in the top border besides the label: `┌─ ` + ` ┐` (5 chars).
const TOP_BORDER_FIXED_COLS: usize = 5;

/// Single-line text input rendered inside a box with the label intersecting the top border.
///
/// ```text
/// ┌─ Name ─────────────────────────┐
/// │ my-agent▏                      │
/// └────────────────────────────────┘
/// ```
pub struct BorderedTextField {
    pub inner: TextField,
    label: String,
    width: usize,
    placeholder: Option<String>,
}

impl BorderedTextField {
    pub fn new(label: impl Into<String>, value: String) -> Self {
        Self { inner: TextField::new(value), label: label.into(), width: 0, placeholder: None }
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn set_width(&mut self, width: usize) {
        self.width = width;
        self.inner.set_content_width(width.saturating_sub(HORIZONTAL_PADDING).max(1));
    }

    pub fn set_value(&mut self, value: String) {
        self.inner.set_value(value);
    }

    pub fn value(&self) -> &str {
        &self.inner.value
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.inner.to_json()
    }

    pub fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        let width = self.width.max(self.min_width());
        let glyphs = BorderGlyphs::for_focus(focused);
        let border_color = if focused { context.theme.primary() } else { context.theme.text_secondary() };
        let border_style = Style::fg(border_color);
        let label_style = Style::fg(context.theme.text_primary());

        vec![
            self.top_border(width, glyphs, border_style, label_style),
            self.middle_row(width, glyphs, border_style, context, focused),
            Self::bottom_border(width, glyphs, border_style),
        ]
    }

    fn min_width(&self) -> usize {
        TOP_BORDER_FIXED_COLS + 1 + display_width_text(&self.label)
    }

    fn top_border(&self, width: usize, glyphs: BorderGlyphs, border_style: Style, label_style: Style) -> Line {
        let label_cols = display_width_text(&self.label);
        let dash_cols = width.saturating_sub(label_cols + TOP_BORDER_FIXED_COLS);

        let mut line = Line::default();
        line.push_with_style(format!("{}{} ", glyphs.top_left, glyphs.horizontal), border_style);
        line.push_with_style(self.label.clone(), label_style);
        line.push_with_style(" ", border_style);
        line.push_with_style(glyphs.horizontal.repeat(dash_cols), border_style);
        line.push_with_style(glyphs.top_right, border_style);
        line
    }

    fn middle_row(
        &self,
        width: usize,
        glyphs: BorderGlyphs,
        border_style: Style,
        context: &ViewContext,
        focused: bool,
    ) -> Line {
        let content_width = width.saturating_sub(HORIZONTAL_PADDING);
        let inner_line = self.placeholder.as_ref().filter(|_| self.inner.value.is_empty()).map_or_else(
            || self.inner.render_field(context, focused).into_iter().next().unwrap_or_default(),
            |placeholder| Line::with_style(placeholder.clone(), Style::fg(context.theme.muted())),
        );
        let clipped = truncate_line(&inner_line, content_width);

        let mut row = Line::default();
        row.push_with_style(format!("{} ", glyphs.vertical), border_style);
        row.append_line(&clipped);
        row.extend_bg_to_width(width.saturating_sub(2));
        row.push_with_style(format!(" {}", glyphs.vertical), border_style);
        row
    }

    fn bottom_border(width: usize, glyphs: BorderGlyphs, border_style: Style) -> Line {
        let inner_dashes = width.saturating_sub(2);
        let mut line = Line::default();
        line.push_with_style(glyphs.bottom_left, border_style);
        line.push_with_style(glyphs.horizontal.repeat(inner_dashes), border_style);
        line.push_with_style(glyphs.bottom_right, border_style);
        line
    }
}

#[derive(Clone, Copy)]
struct BorderGlyphs {
    top_left: &'static str,
    top_right: &'static str,
    bottom_left: &'static str,
    bottom_right: &'static str,
    horizontal: &'static str,
    vertical: &'static str,
}

impl BorderGlyphs {
    const LIGHT: Self =
        Self {
            top_left: "┌", top_right: "┐", bottom_left: "└", bottom_right: "┘", horizontal: "─", vertical: "│"
        };
    const HEAVY: Self =
        Self {
            top_left: "┏", top_right: "┓", bottom_left: "┗", bottom_right: "┛", horizontal: "━", vertical: "┃"
        };

    fn for_focus(focused: bool) -> Self {
        if focused { Self::HEAVY } else { Self::LIGHT }
    }
}

impl Component for BorderedTextField {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        self.inner.on_event(event).await
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}
