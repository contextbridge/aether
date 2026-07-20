use crate::INLINE_SCROLLBACK_RESERVE;
use crate::app::{App, HistoryItem, HistoryKind};
use crate::diff::render_diff;
use crate::markdown::render_markdown;
use crate::presentation::TranscriptRenderer;
use crate::theme::Theme;
use crate::tool_calls::ToolStatus;
use crate::wrap::wrap_line;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

pub fn sync_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    renderer: &mut TranscriptRenderer,
) -> Result<(), B::Error> {
    terminal.autoresize()?;
    let terminal_height = terminal.size()?.height;
    let area = terminal.get_frame().area();
    renderer.sync_transcript_generation(app.transcript_generation());
    let can_insert_history = terminal_height.saturating_sub(area.height) >= INLINE_SCROLLBACK_RESERVE;
    let full_screen_active = app.full_screen_active();

    if can_insert_history && !full_screen_active {
        let previous_kind = app.last_drained_kind();
        let committed = app.drain_finalized();
        let lines =
            renderer.history_lines(&committed, previous_kind, area.width, app.content_padding(), app.spinner_tick());
        renderer.append_committed_lines(lines);
    }

    let pending_line_count = if full_screen_active { 0 } else { live_history_lines(app, renderer, area.width).len() };
    let visible_history_lines = usize::from(live_area_height(area, app, renderer));
    let committed_to_keep = visible_history_lines.saturating_sub(pending_line_count);
    let overflow = if can_insert_history && !full_screen_active {
        renderer.take_committed_overflow(committed_to_keep)
    } else {
        Vec::new()
    };

    terminal.draw(|frame| draw(frame, app, renderer))?;

    if overflow.is_empty() {
        return Ok(());
    }

    for chunk in overflow.chunks(usize::from(u16::MAX)) {
        let chunk = chunk.to_vec();
        let height = u16::try_from(chunk.len()).unwrap_or(u16::MAX);
        terminal.insert_before(height, move |buffer| {
            Paragraph::new(Text::from(chunk)).render(buffer.area, buffer);
        })?;
    }

    terminal.draw(|frame| draw(frame, app, renderer))?;
    Ok(())
}

pub fn draw(frame: &mut Frame, app: &mut App, renderer: &mut TranscriptRenderer) {
    let composer_layout = app.composer().layout(frame.area().width, renderer.theme());
    let overlay_lines = app.composer().overlay_lines(frame.area().width, 6, renderer.theme());
    let requested_composer_height = composer_layout.lines.len().saturating_add(overlay_lines.len());
    let composer_height = resolve_composer_height(requested_composer_height, frame.area().height);
    let [live_area, composer_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(composer_height), Constraint::Length(1)])
            .areas(frame.area());

    let mut lines = renderer.committed_lines().to_vec();
    lines.extend(live_history_lines(app, renderer, live_area.width));
    let visible = live_area.height as usize;
    if lines.len() > visible {
        lines.drain(..lines.len() - visible);
    }
    let rendered_height = u16::try_from(lines.len()).unwrap_or(live_area.height).min(live_area.height);
    let rendered_area =
        Rect { y: live_area.bottom().saturating_sub(rendered_height), height: rendered_height, ..live_area };
    frame.render_widget(Paragraph::new(Text::from(lines)), rendered_area);

    render_composer(frame, &composer_layout, &overlay_lines, composer_area);
    render_status_line(frame, app, status_area, renderer.theme());
    let theme = renderer.theme().clone();
    app.render_modal(frame, &theme, renderer.highlighter());
}

impl TranscriptRenderer {
    pub fn history_lines(
        &mut self,
        items: &[HistoryItem],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            let kind = item.kind();
            if previous.is_some_and(|value| value != kind) {
                lines.push(Line::default());
            }
            lines.extend(self.item_lines(item, width, padding, spinner_tick));
            previous = Some(kind);
        }
        lines
    }

    fn item_lines(
        &mut self,
        item: &HistoryItem,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let content_width = width.saturating_sub(u16::try_from(padding.saturating_mul(2)).unwrap_or(u16::MAX)).max(1);
        let theme = self.theme().clone();
        match item {
            HistoryItem::User(text) => user_block_lines(text, width, padding, &theme),
            HistoryItem::Text(text) => {
                let rendered = render_markdown(text, content_width, &theme, self.highlighter());
                indent_lines(rendered, padding)
            }
            HistoryItem::Thought(text) => indent_lines(
                wrap_line(
                    Line::styled(
                        text.clone(),
                        Style::new().fg(theme.blockquote).add_modifier(Modifier::ITALIC | Modifier::DIM),
                    ),
                    content_width,
                ),
                padding,
            ),
            HistoryItem::Tool { title, status, diff } => {
                let mut lines = tool_lines(title, status, spinner_tick, padding, &theme);
                if matches!(status, ToolStatus::Success)
                    && let Some(preview) = diff
                {
                    let rendered = render_diff(preview, content_width, &theme, self.highlighter());
                    lines.extend(indent_lines(rendered, padding));
                }
                lines
            }
        }
    }
}

fn resolve_composer_height(requested_height: usize, area_height: u16) -> u16 {
    u16::try_from(requested_height).unwrap_or(u16::MAX).max(1).min(area_height.saturating_sub(1))
}

fn live_area_height(area: Rect, app: &App, renderer: &TranscriptRenderer) -> u16 {
    let composer_layout = app.composer().layout(area.width, renderer.theme());
    let overlay_height = app.composer().overlay_lines(area.width, 6, renderer.theme()).len();
    let requested_composer_height = composer_layout.lines.len().saturating_add(overlay_height);
    let composer_height = resolve_composer_height(requested_composer_height, area.height);
    area.height.saturating_sub(composer_height).saturating_sub(1)
}

fn live_history_lines(app: &App, renderer: &mut TranscriptRenderer, width: u16) -> Vec<Line<'static>> {
    renderer.history_lines(
        &app.pending_items(),
        app.last_drained_kind(),
        width,
        app.content_padding(),
        app.spinner_tick(),
    )
}

fn user_block_lines(text: &str, width: u16, padding: usize, theme: &Theme) -> Vec<Line<'static>> {
    let block_style = Style::new().bg(theme.sidebar_bg).fg(theme.text_primary);
    let width = usize::from(width);
    let content_width = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let blank = Line::styled(" ".repeat(width), block_style);
    let mut lines = vec![blank.clone()];
    let text = text.trim_end_matches('\n');
    for wrapped in wrap_text(text, content_width) {
        let fill = width.saturating_sub(padding + wrapped.width());
        lines.push(Line::styled(format!("{}{wrapped}{}", " ".repeat(padding), " ".repeat(fill)), block_style));
    }
    lines.push(blank);
    lines
}

fn tool_lines(
    title: &str,
    status: &ToolStatus,
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (glyph, glyph_style, suffix) = match status {
        ToolStatus::Running => (spinner_frame(spinner_tick), Style::new().fg(theme.info), None),
        ToolStatus::Success => ("✓", Style::new().fg(theme.success), None),
        ToolStatus::Error(cause) => ("✗", Style::new().fg(theme.error), Some(format!(" ({cause})"))),
    };
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        Span::styled(glyph.to_string(), glyph_style),
        Span::raw(" "),
        Span::styled(title.to_string(), Style::new().fg(theme.text_primary)),
    ];
    if let Some(suffix) = suffix {
        spans.push(Span::styled(suffix, Style::new().fg(theme.error)));
    }
    vec![Line::from(spans)]
}

fn indent_lines(lines: Vec<Line<'static>>, padding: usize) -> Vec<Line<'static>> {
    let prefix = " ".repeat(padding);
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw(prefix.clone()));
            line
        })
        .collect()
}

fn render_composer(
    frame: &mut Frame,
    layout: &crate::composer::ComposerLayout,
    overlay_lines: &[Line<'static>],
    area: Rect,
) {
    let mut lines = layout.lines.clone();
    lines.extend_from_slice(overlay_lines);
    let skip = lines.len().saturating_sub(usize::from(area.height));
    let visible: Vec<Line<'static>> = lines.into_iter().skip(skip).collect();
    frame.render_widget(Paragraph::new(Text::from(visible)), area);

    let x = area.x.saturating_add(layout.cursor.x);
    let y = area.y.saturating_add(layout.cursor.y);
    if x < area.right() && y < area.bottom() {
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn render_status_line(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let dim = Style::new().fg(theme.muted);
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
    for character in word.chars() {
        if current.width() + unicode_width::UnicodeWidthChar::width(character).unwrap_or(0) > max_width {
            lines.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    current
}
