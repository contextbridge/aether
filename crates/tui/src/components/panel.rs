use crate::{Color, FitOptions, Frame, Line, Style, ViewContext};
use unicode_width::UnicodeWidthStr;

/// Width consumed by left ("│ ") and right (" │") borders.
pub const BORDER_H_PAD: u16 = 4;

/// A bordered panel for wrapping content blocks with title/footer chrome.
///
/// For borderless stacking with cursor tracking, use [`Frame::vstack`](crate::Frame::vstack).
///
/// # Example
///
/// ```
/// use tui::{Panel, Line, ViewContext};
///
/// let mut panel = Panel::new(tui::Color::Grey)
///     .title(" Settings ")
///     .footer("[Enter] Save [Esc] Cancel")
///     .gap(1);
///
/// panel.push(vec![Line::new("Name: Example")]);
/// panel.push(vec![Line::new("Value: 42")]);
///
/// let ctx = ViewContext::new((40, 20));
/// let frame = panel.render(&ctx);
/// ```
pub struct Panel {
    blocks: Vec<Vec<Line>>,
    title: Option<String>,
    footer: Option<PanelFooter>,
    border_color: Color,
    fill_height: Option<usize>,
    gap: usize,
}

enum PanelFooter {
    Text(String),
    Line(Line),
}

impl Panel {
    pub fn new(border_color: Color) -> Self {
        Self { blocks: Vec::new(), title: None, footer: None, border_color, fill_height: None, gap: 0 }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(PanelFooter::Text(footer.into()));
        self
    }

    pub fn footer_line(mut self, footer: Line) -> Self {
        self.footer = Some(PanelFooter::Line(footer));
        self
    }

    pub fn fill_height(mut self, h: usize) -> Self {
        self.fill_height = Some(h);
        self
    }

    pub fn gap(mut self, lines: usize) -> Self {
        self.gap = lines;
        self
    }

    pub fn push(&mut self, block: Vec<Line>) {
        self.blocks.push(block);
    }

    /// Inner content width when borders are active.
    pub fn inner_width(total_width: u16) -> u16 {
        total_width.saturating_sub(BORDER_H_PAD)
    }

    /// Render blocks with borders/chrome.
    pub fn render(&self, context: &ViewContext) -> Frame {
        let width = context.size.width as usize;
        let inner_width = width.saturating_sub(BORDER_H_PAD as usize);
        let inner_width_u16 = u16::try_from(inner_width).unwrap_or(u16::MAX);
        let border_style = Style::fg(self.border_color);
        let border_left = Line::new("│ ".to_string());
        let border_right = Line::new(" │".to_string());

        let blank_border = || Frame::new(vec![empty_border_line(inner_width)]);

        let title_text = self.title.as_deref().unwrap_or("");
        let bar_left = "┌─";
        let bar_right_pad =
            width.saturating_sub(UnicodeWidthStr::width(bar_left) + UnicodeWidthStr::width(title_text) + 1); // 1 for ┐
        let title_line = format!("{bar_left}{title_text}{:─>bar_right_pad$}┐", "", bar_right_pad = bar_right_pad);
        let top_frame = Frame::new(vec![Line::with_style(title_line, border_style)]);

        let mut body_frames: Vec<Frame> = vec![blank_border()];
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                for _ in 0..self.gap {
                    body_frames.push(blank_border());
                }
            }
            body_frames.push(Frame::new(block.clone()).fit(inner_width_u16, FitOptions::wrap().with_fill()).wrap_each(
                inner_width_u16,
                &border_left,
                &border_right,
            ));
        }
        let mut body_frame = Frame::vstack(body_frames);

        if let Some(target_height) = self.fill_height {
            // Reserve space for top border (1) + footer (0/1) + bottom border (1).
            let chrome_rows = if self.footer.is_some() { 3 } else { 2 };
            let target_body = target_height.saturating_sub(chrome_rows);
            let current = body_frame.lines().len();
            if current < target_body {
                let pad: Vec<Frame> = (0..(target_body - current)).map(|_| blank_border()).collect();
                body_frame = Frame::vstack(std::iter::once(body_frame).chain(pad));
            }
        }

        let mut chrome: Vec<Frame> = Vec::with_capacity(2);
        if let Some(footer) = &self.footer {
            chrome.push(Frame::new(vec![render_footer(footer, inner_width, border_style)]));
        }
        let bottom_inner = width.saturating_sub(2); // └ and ┘
        let bottom_line = format!("└{:─>bottom_inner$}┘", "", bottom_inner = bottom_inner);
        chrome.push(Frame::new(vec![Line::with_style(bottom_line, border_style)]));

        let result = Frame::vstack(std::iter::once(top_frame).chain(std::iter::once(body_frame)).chain(chrome));

        if let Some(target_height) = self.fill_height {
            result.truncate_height(u16::try_from(target_height).unwrap_or(u16::MAX))
        } else {
            result
        }
    }
}

fn render_footer(footer: &PanelFooter, inner_width: usize, border_style: Style) -> Line {
    match footer {
        PanelFooter::Text(text) => {
            let footer_pad = inner_width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
            let footer_line = format!("│ {text}{:footer_pad$} │", "", footer_pad = footer_pad);
            Line::with_style(footer_line, border_style)
        }
        PanelFooter::Line(line) => {
            let footer_pad = inner_width.saturating_sub(line.display_width());
            let mut footer_line = Line::with_style("│ ", border_style);
            footer_line.append_line(line);
            footer_line.push_with_style(format!("{:footer_pad$} │", "", footer_pad = footer_pad), border_style);
            footer_line
        }
    }
}

fn empty_border_line(inner_width: usize) -> Line {
    Line::new(format!("│ {:inner_width$} │", "", inner_width = inner_width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_renders_top_border_with_title_text() {
        let mut container = Panel::new(Color::Grey).title(" Config ");
        container.push(vec![Line::new("x")]);
        let context = ViewContext::new((30, 10));
        let lines = container.render(&context).into_lines();
        let top = lines[0].plain_text();
        assert!(top.starts_with("┌─ Config "), "top: {top}");
        assert!(top.ends_with('┐'), "top: {top}");
    }

    #[test]
    fn footer_renders_footer_and_bottom_border() {
        let mut container = Panel::new(Color::Grey).footer("[Esc] Close");
        container.push(vec![Line::new("x")]);
        let context = ViewContext::new((30, 10));
        let lines = container.render(&context).into_lines();
        let last = lines.last().unwrap().plain_text();
        assert!(last.starts_with('└'), "last: {last}");
        assert!(last.ends_with('┘'), "last: {last}");
        let footer = lines[lines.len() - 2].plain_text();
        assert!(footer.contains("[Esc] Close"), "footer: {footer}");
    }

    #[test]
    fn fill_height_pads_with_empty_bordered_rows() {
        let mut container = Panel::new(Color::Grey).title(" T ").footer("F").fill_height(10);
        container.push(vec![Line::new("x")]);
        let context = ViewContext::new((30, 10));
        let lines = container.render(&context).into_lines();
        assert_eq!(lines.len(), 10, "should fill to exactly 10 lines");
    }

    #[test]
    fn border_color_styles_border_lines() {
        let mut container = Panel::new(Color::Cyan).title(" T ");
        container.push(vec![Line::new("x")]);
        let context = ViewContext::new((30, 10));
        let lines = container.render(&context).into_lines();
        // Top border should have Cyan fg
        let top_span = &lines[0].spans()[0];
        assert_eq!(top_span.style().fg, Some(Color::Cyan));
        // Bottom border should have Cyan fg
        let bottom_span = &lines.last().unwrap().spans()[0];
        assert_eq!(bottom_span.style().fg, Some(Color::Cyan));
    }

    #[test]
    fn bg_color_extends_through_padding() {
        let bg = Color::DarkBlue;
        let mut container = Panel::new(Color::Grey);
        container.push(vec![Line::with_style("hi", Style::default().bg_color(bg))]);
        let context = ViewContext::new((20, 10));
        let lines = container.render(&context).into_lines();
        // Content row (top border + blank + first content = index 2)
        let content_row = &lines[2];
        let bg_span =
            content_row.spans().iter().find(|s| s.style().bg == Some(bg)).expect("should have a span with bg color");
        assert!(bg_span.text().len() > 2, "bg span should extend through padding, got: {:?}", bg_span.text());
    }

    #[test]
    fn bordered_gap_inserts_empty_bordered_lines_between_children() {
        let mut container = Panel::new(Color::Grey).gap(1);
        container.push(vec![Line::new("a")]);
        container.push(vec![Line::new("b")]);
        let context = ViewContext::new((20, 10));
        let lines = container.render(&context).into_lines();
        // top border + blank + "a" + gap_blank + "b" + bottom border = 6
        assert_eq!(lines.len(), 6);
        let gap_line = lines[3].plain_text();
        assert!(gap_line.starts_with('│'), "gap: {gap_line}");
        assert!(gap_line.ends_with('│'), "gap: {gap_line}");
    }

    #[test]
    fn overlong_content_wraps_inside_borders() {
        let mut container = Panel::new(Color::Grey);
        container.push(vec![Line::new("abcdefghijklmnop")]);
        // total width 14 → inner width 10 (14 − BORDER_H_PAD)
        let context = ViewContext::new((14, 10));
        let lines = container.render(&context).into_lines();

        // top border + blank + 2 wrapped content rows + bottom border = 5
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[2].plain_text(), "│ abcdefghij │");
        assert_eq!(lines[3].plain_text(), "│ klmnop     │");
    }

    #[test]
    fn top_and_bottom_border_have_equal_visual_width() {
        let mut container = Panel::new(Color::Grey).title(" Config ");
        container.push(vec![Line::new("x")]);
        let context = ViewContext::new((40, 10));
        let lines = container.render(&context).into_lines();
        let top = lines.first().unwrap().plain_text();
        let bottom = lines.last().unwrap().plain_text();
        assert_eq!(
            UnicodeWidthStr::width(top.as_str()),
            UnicodeWidthStr::width(bottom.as_str()),
            "top ({top}) and bottom ({bottom}) border should have equal visual width"
        );
    }
}
