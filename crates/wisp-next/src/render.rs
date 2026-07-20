use crate::app::{App, HistoryItem, HistoryKind};
use crate::composer::Composer;
use crate::tool_calls::ToolStatus;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

pub const MAX_COMPOSER_HEIGHT: u16 = 5;

/// Push newly-finalized history into terminal scrollback, then redraw the
/// live viewport. This is the single render entry point for both the real
/// event loop and `TestBackend`-driven tests.
pub fn sync_terminal<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error> {
    let width = terminal.size()?.width;
    let prev_kind = app.last_drained_kind();
    let drained = app.drain_finalized();

    if !drained.is_empty() {
        let lines = history_lines(&drained, prev_kind, width, app.content_padding(), app.spinner_tick());
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        terminal.insert_before(height, |buf| {
            Paragraph::new(Text::from(lines)).render(buf.area, buf);
        })?;
    }

    terminal.draw(|frame| draw(frame, app))?;
    Ok(())
}

/// Render the fixed inline viewport: live (still-mutating) history at the
/// top, the prompt composer below it, and a one-row status line at the bottom.
pub fn draw(frame: &mut Frame, app: &App) {
    let composer_height = u16::try_from(app.composer().line_count()).unwrap_or(u16::MAX).clamp(1, MAX_COMPOSER_HEIGHT);
    let [live_area, composer_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(composer_height), Constraint::Length(1)])
            .areas(frame.area());

    let items = app.pending_items();
    let mut lines =
        history_lines(&items, app.last_drained_kind(), live_area.width, app.content_padding(), app.spinner_tick());
    let visible = live_area.height as usize;
    if lines.len() > visible {
        lines.drain(..lines.len() - visible);
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), live_area);

    render_composer(frame, app.composer(), composer_area);
    render_status_line(frame, app, status_area);
}

/// Render history items to display lines, wrapped at the given width.
/// Adjacent items of differing kinds are separated by one blank line;
/// `prev_kind` carries that rule across the scrollback/live boundary.
pub fn history_lines(
    items: &[HistoryItem],
    prev_kind: Option<HistoryKind>,
    width: u16,
    padding: usize,
    spinner_tick: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut prev = prev_kind;

    for item in items {
        let kind = item.kind();
        if prev.is_some_and(|p| p != kind) {
            lines.push(Line::default());
        }
        lines.extend(item_lines(item, width, padding, spinner_tick));
        prev = Some(kind);
    }

    lines
}

fn item_lines(item: &HistoryItem, width: u16, padding: usize, spinner_tick: usize) -> Vec<Line<'static>> {
    let content_width = (width as usize).saturating_sub(padding * 2).max(1);
    let indent = " ".repeat(padding);

    match item {
        HistoryItem::User(text) => user_block_lines(text, width, padding),
        HistoryItem::Text(text) => {
            wrap_text(text, content_width).into_iter().map(|line| Line::from(format!("{indent}{line}"))).collect()
        }
        HistoryItem::Thought(text) => wrap_text(text, content_width)
            .into_iter()
            .map(|line| {
                Line::styled(format!("{indent}{line}"), Style::new().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))
            })
            .collect(),
        HistoryItem::Tool { title, status } => {
            let (glyph, glyph_style, suffix) = match status {
                ToolStatus::Running => (spinner_frame(spinner_tick), Style::new().fg(Color::Cyan), None),
                ToolStatus::Success => ("⏺", Style::new().fg(Color::Green), None),
                ToolStatus::Error(cause) => ("✗", Style::new().fg(Color::Red), Some(format!(" ({cause})"))),
            };
            let mut spans = vec![
                Span::raw(indent),
                Span::styled(glyph.to_string(), glyph_style),
                Span::raw(" "),
                Span::raw(title.clone()),
            ];
            if let Some(suffix) = suffix {
                spans.push(Span::styled(suffix, Style::new().fg(Color::DarkGray)));
            }
            vec![Line::from(spans)]
        }
    }
}

fn user_block_lines(text: &str, width: u16, padding: usize) -> Vec<Line<'static>> {
    let block_style = Style::new().bg(Color::DarkGray).fg(Color::White);
    let width = width as usize;
    let content_width = width.saturating_sub(padding * 2).max(1);
    let blank = Line::styled(" ".repeat(width), block_style);

    let mut lines = vec![blank.clone()];
    for wrapped in wrap_text(text, content_width) {
        let fill = width.saturating_sub(padding + wrapped.width());
        lines.push(Line::styled(format!("{}{wrapped}{}", " ".repeat(padding), " ".repeat(fill)), block_style));
    }
    lines.push(blank);
    lines
}

fn render_composer(frame: &mut Frame, composer: &Composer, area: Rect) {
    let visible = area.height as usize;
    let skip = composer.line_count().saturating_sub(visible);

    let lines: Vec<Line> = composer
        .lines()
        .enumerate()
        .skip(skip)
        .map(|(index, line)| {
            let prefix = if index == 0 { "> " } else { "  " };
            Line::from(vec![Span::styled(prefix, Style::new().fg(Color::Cyan)), Span::raw(line.to_string())])
        })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);

    let (row, column) = composer.cursor_position();
    if row >= skip {
        let x = area.x + u16::try_from(2 + column).unwrap_or(u16::MAX);
        let y = area.y + u16::try_from(row - skip).unwrap_or(u16::MAX);
        if x < area.right() && y < area.bottom() {
            frame.set_cursor_position(Position::new(x, y));
        }
    }
}

fn render_status_line(frame: &mut Frame, app: &App, area: Rect) {
    let dim = Style::new().fg(Color::DarkGray);
    frame.render_widget(Paragraph::new(app.workspace_label()).style(dim), area);

    let right = if app.exit_confirmation_active() {
        "press ctrl-c again to exit".to_string()
    } else {
        let mut parts = Vec::new();
        if app.busy() {
            parts.push(format!("{} working · esc to cancel", spinner_frame(app.spinner_tick())));
        }
        parts.push(app.agent_name().to_string());
        if let Some(percent) = app.context_percent() {
            parts.push(format!("ctx {percent}%"));
        }
        parts.join(" · ")
    };
    frame.render_widget(Paragraph::new(right).style(dim).alignment(Alignment::Right), area);
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for raw_line in text.split('\n') {
        if raw_line.width() <= max_width {
            lines.push(raw_line.to_string());
            continue;
        }

        let mut current = String::new();
        for word in raw_line.split(' ') {
            let needed = if current.is_empty() { word.width() } else { current.width() + 1 + word.width() };
            if needed <= max_width {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            } else {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current = break_long_word(word, max_width, &mut lines);
            }
        }
        lines.push(current);
    }

    lines
}

fn break_long_word(word: &str, max_width: usize, lines: &mut Vec<String>) -> String {
    let mut current = String::new();
    for c in word.chars() {
        if current.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) > max_width {
            lines.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    current
}
