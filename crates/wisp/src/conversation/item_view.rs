use crate::theme::Theme;
use crate::view::markdown::render_markdown;
use crate::view::syntax::SyntaxHighlighter;
use crate::view::wrap::wrap_text;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::tool_view::tool_lines;
use acp_utils::conversation::{ConversationContent, ConversationItem};

/// Which of the four transcript content kinds an item holds, driving the blank
/// line inserted between runs of different kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentKind {
    User,
    Assistant,
    Tool,
    Notice,
}

/// One item's rendered rows, whatever it holds.
pub(crate) fn item_lines(
    item: &ConversationItem,
    width: u16,
    padding: usize,
    spinner_tick: usize,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let content_width = content_width(width, padding);
    if let ConversationContent::Tool(tool) = item.content() {
        return tool_lines(tool, content_width, padding, spinner_tick, theme, highlighter);
    }
    let text = item.text().unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    match item.content() {
        ConversationContent::Assistant(_) => {
            indent_lines(render_markdown(&text, content_width, theme, highlighter), padding)
        }
        _ => user_block_lines(&text, width, padding, theme),
    }
}

pub(crate) fn content_kind(item: &ConversationItem) -> ContentKind {
    match item.content() {
        ConversationContent::User(_) => ContentKind::User,
        ConversationContent::Assistant(_) => ContentKind::Assistant,
        ConversationContent::Tool(_) => ContentKind::Tool,
        ConversationContent::Notice(_) => ContentKind::Notice,
    }
}

/// Width left for content once the gutter is taken from both sides.
pub(crate) fn content_width(width: u16, padding: usize) -> u16 {
    width.saturating_sub(u16::try_from(padding.saturating_mul(2)).unwrap_or(u16::MAX)).max(1)
}

/// Prepends the content gutter to every row.
pub(crate) fn indent_lines(lines: Vec<Line<'static>>, padding: usize) -> Vec<Line<'static>> {
    let prefix = " ".repeat(padding);
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw(prefix.clone()));
            line
        })
        .collect()
}

/// The user's own message, drawn as a full-width tinted block.
fn user_block_lines(text: &str, width: u16, padding: usize, theme: &Theme) -> Vec<Line<'static>> {
    let block_style = theme.surface_style();
    let width = usize::from(width);
    let content_width = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let blank = Line::styled(" ".repeat(width), block_style);
    let mut lines = vec![blank.clone()];
    for wrapped in wrap_text(text.trim_end_matches('\n'), content_width) {
        let fill = width.saturating_sub(padding + wrapped.width());
        lines.push(Line::styled(format!("{}{wrapped}{}", " ".repeat(padding), " ".repeat(fill)), block_style));
    }
    lines.push(blank);
    lines
}
