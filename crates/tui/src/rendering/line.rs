use crossterm::style::{Attribute, Color, SetAttribute, SetBackgroundColor, SetForegroundColor};
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

use super::soft_wrap;
use super::span::Span;
use super::style::Style;

#[doc = include_str!("../docs/line.md")]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    spans: Vec<Span>,
    /// Optional row-fill background. When `Some`, materialization at
    /// composition (`Frame::hstack`) or at the terminal boundary
    /// (`VisualFrame::from_frame`) will paint trailing columns of the
    /// containing slot with this color. Deferring materialization prevents
    /// premature trailing-space rows from producing phantom wrapped rows when
    /// wrapped again at a smaller width.
    fill: Option<Color>,
}

impl Line {
    pub fn new(s: impl Into<String>) -> Self {
        let text = s.into();
        if text.is_empty() {
            return Self::default();
        }

        Self { spans: vec![Span::new(text)], fill: None }
    }

    pub fn styled(text: impl Into<String>, color: Color) -> Self {
        Self::with_style(text, Style::fg(color))
    }

    pub fn with_style(text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        if text.is_empty() {
            return Self::default();
        }

        Self { spans: vec![Span::with_style(text, style)], fill: None }
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.fill.is_none()
    }

    /// Returns this row's fill background, if any.
    pub fn fill(&self) -> Option<Color> {
        self.fill
    }

    /// Builder: mark this row as filling its containing width with `color`.
    pub fn with_fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }

    /// Set or clear this row's fill background. Pass `Some(color)` to mark
    /// the row for fill, or `None` to drop any existing fill metadata.
    pub fn set_fill(&mut self, fill: Option<Color>) {
        self.fill = fill;
    }

    /// The background color this row's trailing space *would* be filled with.
    ///
    /// Prefers explicit fill metadata, otherwise the first background color
    /// found among the row's spans, otherwise `None`. Used by composition
    /// layers (e.g., `Frame::fit` with `with_fill`) to decide what background
    /// to extend.
    pub fn infer_fill_color(&self) -> Option<Color> {
        self.fill.or_else(|| self.spans.iter().find_map(|s| s.style().bg))
    }

    pub fn prepend(mut self, text: impl Into<String>) -> Self {
        let text = text.into();

        if text.is_empty() {
            return self;
        }

        // If a fill style is set, or the *leading* span has a bg, prepended
        // text picks that style up so the indent is visually contiguous with
        // the row's eventual fill. Looking only at the first span avoids
        // bleeding a later span's bg (e.g. a diff_added_bg content span)
        // backwards across a no-bg gutter into the prepended indent.
        // Otherwise, merge into the leading default-style span when possible
        // to keep span counts low.
        let bg_color = self.fill.or_else(|| self.spans.first().and_then(|s| s.style().bg));

        if let Some(bg) = bg_color {
            self.spans.insert(0, Span::with_style(text, Style::default().bg_color(bg)));
        } else if let Some(first) = self.spans.first_mut()
            && first.style == Style::default()
        {
            first.text.insert_str(0, &text);
        } else {
            self.spans.insert(0, Span::with_style(text, Style::default()));
        }

        self
    }

    pub fn push_text(&mut self, text: impl Into<String>) {
        self.push_span(Span::new(text));
    }

    pub fn push_styled(&mut self, text: impl Into<String>, color: Color) {
        self.push_with_style(text, Style::fg(color));
    }

    pub fn push_with_style(&mut self, text: impl Into<String>, style: Style) {
        self.push_span(Span::with_style(text, style));
    }

    pub fn push_span(&mut self, span: Span) {
        if span.text.is_empty() {
            return;
        }

        if let Some(last) = self.spans.last_mut()
            && last.style == span.style
        {
            last.text.push_str(&span.text);
            return;
        }
        self.spans.push(span);
    }

    pub fn append_line(&mut self, other: &Line) {
        for span in &other.spans {
            self.push_span(span.clone());
        }
    }

    pub fn extend_bg_to_width(&mut self, target_width: usize) {
        let current_width = UnicodeWidthStr::width(self.plain_text().as_str());
        let pad = target_width.saturating_sub(current_width);
        if pad == 0 {
            self.fill = None;
            return;
        }

        let pad_style = self.infer_fill_color().map_or_else(Style::default, |bg| Style::default().bg_color(bg));
        self.fill = None;
        self.push_with_style(format!("{:pad$}", "", pad = pad), pad_style);
    }

    pub fn to_ansi_string(&self) -> String {
        if self.spans.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        let mut active_style = Style::default();

        for span in &self.spans {
            if span.style != active_style {
                emit_style_transition(&mut out, active_style, span.style);
                active_style = span.style;
            }
            out.push_str(&span.text);
        }

        if active_style != Style::default() {
            emit_style_transition(&mut out, active_style, Style::default());
        }

        out
    }

    /// Display width in terminal columns (accounts for unicode widths).
    pub fn display_width(&self) -> usize {
        soft_wrap::display_width_line(self)
    }

    /// Soft-wrap this line to fit within `width` columns.
    pub fn soft_wrap(&self, width: u16) -> Vec<Line> {
        soft_wrap::soft_wrap_line(self, width)
    }

    #[allow(dead_code)]
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for span in &self.spans {
            text.push_str(&span.text);
        }
        text
    }
}

impl std::fmt::Display for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for span in &self.spans {
            f.write_str(&span.text)?;
        }
        Ok(())
    }
}

fn push_fg_sgr(out: &mut String, color: Option<Color>) {
    let fg = color.unwrap_or(Color::Reset);
    let _ = write!(out, "{}", SetForegroundColor(fg));
}

fn push_bg_sgr(out: &mut String, color: Option<Color>) {
    let bg = color.unwrap_or(Color::Reset);
    let _ = write!(out, "{}", SetBackgroundColor(bg));
}

fn push_attr_sgr(out: &mut String, attr: Attribute) {
    let _ = write!(out, "{}", SetAttribute(attr));
}

fn emit_style_transition(out: &mut String, from: Style, to: Style) {
    // Check if any boolean attribute turned OFF — requires a full reset
    let needs_reset = (from.bold && !to.bold)
        || (from.italic && !to.italic)
        || (from.underline && !to.underline)
        || (from.dim && !to.dim)
        || (from.strikethrough && !to.strikethrough);

    if needs_reset {
        push_attr_sgr(out, Attribute::Reset);
        // After reset, re-emit all active attributes and colors on `to`
        if to.bold {
            push_attr_sgr(out, Attribute::Bold);
        }
        if to.italic {
            push_attr_sgr(out, Attribute::Italic);
        }
        if to.underline {
            push_attr_sgr(out, Attribute::Underlined);
        }
        if to.dim {
            push_attr_sgr(out, Attribute::Dim);
        }
        if to.strikethrough {
            push_attr_sgr(out, Attribute::CrossedOut);
        }
        push_fg_sgr(out, to.fg);
        push_bg_sgr(out, to.bg);
        return;
    }

    // Only turning attributes ON — emit incrementally
    if !from.bold && to.bold {
        push_attr_sgr(out, Attribute::Bold);
    }
    if !from.italic && to.italic {
        push_attr_sgr(out, Attribute::Italic);
    }
    if !from.underline && to.underline {
        push_attr_sgr(out, Attribute::Underlined);
    }
    if !from.dim && to.dim {
        push_attr_sgr(out, Attribute::Dim);
    }
    if !from.strikethrough && to.strikethrough {
        push_attr_sgr(out, Attribute::CrossedOut);
    }
    if from.fg != to.fg {
        push_fg_sgr(out, to.fg);
    }
    if from.bg != to.bg {
        push_bg_sgr(out, to.bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_merges_into_default_style_span() {
        let line = Line::new("hello").prepend("  ");
        assert_eq!(line.plain_text(), "  hello");
        assert_eq!(line.spans().len(), 1, "should merge into the existing span");
    }

    #[test]
    fn prepend_carries_bg_from_styled_span() {
        let line = Line::with_style("hello", Style::default().bg_color(Color::Blue));
        let prepended = line.prepend("  ");
        assert_eq!(prepended.plain_text(), "  hello");
        assert_eq!(prepended.spans().len(), 2);
        assert_eq!(prepended.spans()[0].style().bg, Some(Color::Blue), "prepended span should inherit the bg color");
    }

    #[test]
    fn prepend_empty_is_noop() {
        let line = Line::new("hello").prepend("");
        assert_eq!(line.plain_text(), "hello");
    }

    #[test]
    fn with_fill_sets_fill_metadata_without_changing_spans() {
        let line = Line::new("hello").with_fill(Color::Red);
        assert_eq!(line.plain_text(), "hello");
        assert_eq!(line.fill(), Some(Color::Red));
    }

    #[test]
    fn fill_defaults_to_none() {
        let line = Line::new("hello");
        assert_eq!(line.fill(), None);
    }

    #[test]
    fn extend_bg_to_width_consumes_fill_and_uses_its_color_for_padding() {
        let mut line = Line::new("hi").with_fill(Color::Magenta);
        line.extend_bg_to_width(5);
        assert_eq!(line.plain_text(), "hi   ");
        assert_eq!(line.fill(), None);
        let pad_span = line.spans().last().unwrap();
        assert_eq!(pad_span.style().bg, Some(Color::Magenta));
    }

    #[test]
    fn extend_bg_to_width_clears_fill_when_already_at_target_width() {
        let mut line = Line::new("hello").with_fill(Color::Red);
        line.extend_bg_to_width(5);
        assert_eq!(line.plain_text(), "hello");
        assert_eq!(line.fill(), None, "fill should be cleared even when no padding was needed");
    }

    #[test]
    fn extend_bg_to_width_falls_back_to_span_bg_when_no_fill_set() {
        let mut line = Line::with_style("hi", Style::default().bg_color(Color::Blue));
        line.extend_bg_to_width(5);
        let pad_span = line.spans().last().unwrap();
        assert_eq!(pad_span.style().bg, Some(Color::Blue));
    }

    #[test]
    fn prepend_carries_fill_color_when_no_span_bg_present() {
        let line = Line::new("hi").with_fill(Color::Green).prepend("..");
        assert_eq!(line.plain_text(), "..hi");
        // Prepend should not produce a default-style span; it should pick up the
        // fill color so the indent is visually contiguous with the row's fill.
        assert_eq!(line.spans()[0].style().bg, Some(Color::Green));
    }

    #[test]
    fn prepend_does_not_inherit_bg_from_non_leading_span() {
        // A line built like a split-diff row: a no-bg gutter followed by
        // colored content. Prepending an indent must NOT inherit the
        // content span's bg, otherwise the indent visibly leaks the diff
        // color out the left edge of the line.
        let mut line = Line::default();
        line.push_text("     ");
        line.push_with_style("old code", Style::default().bg_color(Color::Red));
        let prepended = line.prepend("  ");

        assert_eq!(prepended.plain_text(), "       old code");
        assert_eq!(prepended.spans()[0].style().bg, None, "prepended indent should not pick up bg from a later span");
    }

    #[test]
    fn builder_style_supports_bold_and_color() {
        let mut line = Line::default();
        line.push_with_style("hot", Style::default().bold().color(Color::Red));

        let ansi = line.to_ansi_string();
        let mut bold = String::new();
        let mut red = String::new();
        let mut reset_attr = String::new();
        push_attr_sgr(&mut bold, Attribute::Bold);
        push_fg_sgr(&mut red, Some(Color::Red));
        push_attr_sgr(&mut reset_attr, Attribute::Reset);

        assert!(ansi.contains(&bold));
        assert!(ansi.contains(&red));
        assert!(ansi.contains("hot"));
        // When bold turns off, a full Reset is emitted
        assert!(ansi.contains(&reset_attr));
    }
}
