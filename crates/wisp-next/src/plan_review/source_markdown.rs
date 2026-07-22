use crate::markdown::InlineMarkdownSpanBuilder;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use pulldown_cmark::{Event, Options, Parser};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub struct SourceMarkdownLine {
    pub source_line_no: usize,
    pub line: Line<'static>,
}

pub fn render_markdown_source_lines(
    text: &str,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<SourceMarkdownLine> {
    let raw_lines: Vec<&str> = text.split('\n').collect();
    let mut rendered = Vec::with_capacity(raw_lines.len());
    let mut index = 0usize;

    while index < raw_lines.len() {
        let raw = raw_lines[index];
        if let Some(fence) = FenceDelimiter::parse(raw) {
            rendered.push(SourceMarkdownLine {
                source_line_no: index + 1,
                line: Line::styled(raw.to_string(), Style::new().fg(theme.muted)),
            });
            index += 1;

            let code_start = index;
            while index < raw_lines.len() && FenceDelimiter::parse(raw_lines[index]).is_none() {
                index += 1;
            }
            let code_end = index;
            let code_text: String = raw_lines[code_start..code_end].join("\n");
            if !code_text.is_empty() {
                let highlighted_lines = highlighter.highlight(&code_text, &fence.lang, theme);
                for (offset, line) in highlighted_lines.into_iter().enumerate() {
                    let styled_line = Line::from(
                        line.spans
                            .into_iter()
                            .map(|span| {
                                Span::styled(span.content.to_string(), span.style.patch(Style::new().bg(theme.code_bg)))
                            })
                            .collect::<Vec<_>>(),
                    );
                    rendered.push(SourceMarkdownLine { source_line_no: code_start + offset + 1, line: styled_line });
                }
            }

            if index < raw_lines.len() {
                rendered.push(SourceMarkdownLine {
                    source_line_no: index + 1,
                    line: Line::styled(raw_lines[index].to_string(), Style::new().fg(theme.muted)),
                });
                index += 1;
            }
        } else {
            rendered
                .push(SourceMarkdownLine { source_line_no: index + 1, line: render_single_markdown_line(raw, theme) });
            index += 1;
        }
    }

    rendered
}

fn render_single_markdown_line(raw: &str, theme: &Theme) -> Line<'static> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Line::default();
    }

    if is_table_source_line(raw) {
        return Line::raw(raw.to_string());
    }

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let mut inline = InlineMarkdownSpanBuilder::new(theme);

    for event in Parser::new_ext(raw, options) {
        match event {
            Event::Start(tag) => inline.start(&tag),
            Event::End(tag) => inline.end(tag),
            Event::Text(text) => inline.push_text(text.into_string()),
            Event::Code(code) => inline.push_code(code.into_string()),
            Event::SoftBreak => inline.push_soft_break(),
            _ => {}
        }
    }

    if inline.is_empty() { Line::raw(raw.to_string()) } else { Line::from(inline.take_spans()) }
}

fn is_table_source_line(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 2
}

struct FenceDelimiter {
    lang: String,
}

impl FenceDelimiter {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim_start();
        let marker = if trimmed.starts_with("```") {
            "```"
        } else if trimmed.starts_with("~~~") {
            "~~~"
        } else {
            return None;
        };

        let rest = trimmed.trim_start_matches(marker).trim();
        Some(Self { lang: rest.split_whitespace().next().unwrap_or("").to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::SyntaxHighlighter;
    use crate::theme::Theme;
    use ratatui::style::Modifier;

    #[test]
    fn renders_plain_text() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("hello world", &theme, &mut highlighter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_line_no, 1);
        assert!(!result[0].line.spans.is_empty());
    }

    #[test]
    fn preserves_source_line_mapping() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("line1\nline2\nline3", &theme, &mut highlighter);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].source_line_no, 1);
        assert_eq!(result[1].source_line_no, 2);
        assert_eq!(result[2].source_line_no, 3);
    }

    #[test]
    fn handles_empty_lines() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("text\n\nmore", &theme, &mut highlighter);
        assert_eq!(result.len(), 3);
        assert_eq!(result[1].source_line_no, 2);
        assert!(result[1].line.spans.iter().all(|s| s.content.is_empty()));
    }

    #[test]
    fn renders_headings_with_style() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("# Hello", &theme, &mut highlighter);
        assert_eq!(result.len(), 1);
        let text: String = result[0].line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Hello"));
    }

    #[test]
    fn renders_bold_and_italic_with_nested_styles() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("**bold and *italic***", &theme, &mut highlighter);

        let spans = &result[0].line.spans;
        assert_eq!(spans.iter().map(|span| span.content.as_ref()).collect::<String>(), "bold and italic");
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
    }

    #[test]
    fn renders_inline_code() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("use `code` here", &theme, &mut highlighter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn handles_fenced_code_blocks() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("```rust\nfn main() {}\n```", &theme, &mut highlighter);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].source_line_no, 1);
        assert_eq!(result[1].source_line_no, 2);
        assert_eq!(result[2].source_line_no, 3);
    }

    #[test]
    fn passes_table_lines_through() {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let result = render_markdown_source_lines("| a | b |", &theme, &mut highlighter);
        assert_eq!(result.len(), 1);
        let text: String = result[0].line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("| a | b |"));
    }
}
