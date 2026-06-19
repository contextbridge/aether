use tui::{FitOptions, Frame, Line, Style, Theme, ViewContext};

pub struct ThoughtMessage<'a> {
    pub text: &'a str,
}

impl ThoughtMessage<'_> {
    fn format_line(text: &str, theme: &Theme) -> Line {
        Line::with_style(text, Style::fg(theme.muted()).italic())
    }

    fn format_lines(text: &str, theme: &Theme) -> Vec<Line> {
        text.lines().map(|line| Self::format_line(line, theme)).collect()
    }
}

impl ThoughtMessage<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        if self.text.is_empty() {
            return Frame::empty();
        }

        let lines = Self::format_lines(self.text, &context.theme);
        Frame::new(lines).fit(context.size.width, FitOptions::wrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_italic_muted_thought_line() {
        let component = ThoughtMessage { text: "check plan" };
        let context = ViewContext::new((80, 24));
        let frame = component.render(&context);
        let lines = frame.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "check plan");
        let style = lines[0].spans()[0].style();
        assert_eq!(style.fg, Some(context.theme.muted()));
        assert!(style.italic);
    }

    #[test]
    fn renders_all_lines_as_italic_muted() {
        let component = ThoughtMessage { text: "line one\nline two" };
        let context = ViewContext::new((80, 24));
        let frame = component.render(&context);
        let lines = frame.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].plain_text(), "line one");
        assert_eq!(lines[1].plain_text(), "line two");
        for line in lines {
            let style = line.spans()[0].style();
            assert_eq!(style.fg, Some(context.theme.muted()));
            assert!(style.italic);
        }
    }

    #[test]
    fn wrapped_continuation_rows_remain_italic_muted() {
        let component = ThoughtMessage { text: "abcdefghijklmnopqrstuvwxyz" };
        let context = ViewContext::new((80, 24));
        let frame = component.render(&context);
        let wrapped = frame.lines()[0].soft_wrap(12);
        assert!(wrapped.len() > 1);

        for row in wrapped.iter().skip(1) {
            assert!(!row.spans().is_empty());
            assert!(
                row.spans()
                    .iter()
                    .all(|span| { span.style().fg == Some(context.theme.muted()) && span.style().italic })
            );
        }
    }
}
