use super::headings::{MarkdownHeading, parse_markdown_headings};
use super::renderer::render_markdown_result;
use crate::line::Line;
use crate::rendering::render_context::ViewContext;
use crate::style::Style;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMarkdownLine {
    pub source_line_no: usize,
    pub line: Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMarkdownRenderResult {
    pub lines: Vec<SourceMarkdownLine>,
    pub headings: Vec<MarkdownHeading>,
}

/// Renders markdown while preserving a 1:1 mapping from rendered rows to source
/// lines, which the block-oriented [`render_markdown_result`] cannot provide.
pub fn render_markdown_source_lines(text: &str, ctx: &ViewContext) -> SourceMarkdownRenderResult {
    let raw_lines = text.split('\n').collect::<Vec<_>>();
    SourceMarkdownRenderResult { lines: render_source_lines(&raw_lines, ctx), headings: parse_markdown_headings(text) }
}

fn render_source_lines(raw_lines: &[&str], ctx: &ViewContext) -> Vec<SourceMarkdownLine> {
    let mut rendered = Vec::with_capacity(raw_lines.len());
    let mut index = 0usize;

    while index < raw_lines.len() {
        let raw = raw_lines[index];
        if let Some(fence) = FenceDelimiter::parse(raw) {
            rendered.push(SourceMarkdownLine {
                source_line_no: index + 1,
                line: render_raw_line(raw, Style::fg(ctx.theme.muted())),
            });
            index += 1;

            let code_start = index;
            while index < raw_lines.len() && FenceDelimiter::parse(raw_lines[index]).is_none() {
                index += 1;
            }
            let code_end = index;
            rendered.extend(render_fenced_code_block(code_start, &raw_lines[code_start..code_end], &fence.lang, ctx));

            if index < raw_lines.len() {
                rendered.push(SourceMarkdownLine {
                    source_line_no: index + 1,
                    line: render_raw_line(raw_lines[index], Style::fg(ctx.theme.muted())),
                });
                index += 1;
            }
        } else {
            rendered
                .push(SourceMarkdownLine { source_line_no: index + 1, line: render_single_markdown_line(raw, ctx) });
            index += 1;
        }
    }

    rendered
}

fn render_single_markdown_line(raw: &str, ctx: &ViewContext) -> Line {
    if raw.trim().is_empty() {
        return Line::default();
    }

    if is_table_source_line(raw) {
        return Line::new(raw);
    }

    render_markdown_result(raw, ctx)
        .lines
        .into_iter()
        .next()
        .map(|line| line.line)
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| Line::new(raw))
}

fn render_raw_line(raw: &str, style: Style) -> Line {
    Line::with_style(raw, style)
}

fn render_fenced_code_block(
    source_start_index: usize,
    raw_lines: &[&str],
    lang: &str,
    ctx: &ViewContext,
) -> Vec<SourceMarkdownLine> {
    if raw_lines.is_empty() {
        return Vec::new();
    }

    let code_text = raw_lines.join("\n");
    let highlighted = ctx.highlighter().highlight(&code_text, lang, &ctx.theme);
    let lines = if highlighted.len() == raw_lines.len() {
        highlighted
    } else {
        raw_lines.iter().map(|raw| render_raw_line(raw, Style::fg(ctx.theme.code_fg()))).collect()
    };

    lines
        .into_iter()
        .enumerate()
        .map(|(offset, line)| SourceMarkdownLine { source_line_no: source_start_index + offset + 1, line })
        .collect()
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
