use crate::INLINE_SCROLLBACK_RESERVE;
use crate::app::{App, HistoryItem, HistoryKind, SubAgentHistoryItem};
use crate::diff::render_diff;
use crate::markdown::render_markdown;
use crate::plan_view::render_plan_lines;
use crate::presentation::TranscriptRenderer;
use crate::settings::{ContextUsageDisplay, ResolvedStatusLineSettings, StatusLineSegmentConfig, StatusLineStyle};
use crate::theme::Theme;
use crate::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, ToolStatus};
use crate::wrap::wrap_line;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{self as acp, SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;
use utils::ReasoningEffort;

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
    let theme = renderer.theme().clone();
    let settings = renderer.settings().clone();
    let status_line_rows = status_line_height(app, frame.area().width, &settings, &theme);
    let composer_layout = app.composer().layout(frame.area().width, &theme);
    let overlay_lines = app.composer().overlay_lines(frame.area().width, 6, &theme);
    let prompt_search_lines = app.composer().prompt_search_lines(frame.area().width, 12, &theme);
    let requested_composer_height =
        composer_layout.lines.len().saturating_add(overlay_lines.len()).saturating_add(prompt_search_lines.len());
    let composer_height = resolve_composer_height(requested_composer_height, frame.area().height, status_line_rows);

    let plan_lines = {
        let padding = app.content_padding();
        render_plan_lines(app.plan_entries(), &theme, padding)
    };
    let plan_height = u16::try_from(plan_lines.len()).unwrap_or(0);
    let remaining = frame.area().height.saturating_sub(composer_height).saturating_sub(status_line_rows);
    let clipped_plan_height = plan_height.min(remaining.div_ceil(3));

    let [live_area, composer_area, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(composer_height),
        Constraint::Length(status_line_rows),
    ])
    .areas(frame.area());

    let visible_plan_lines: Vec<Line<'static>> = plan_lines.into_iter().take(clipped_plan_height as usize).collect();
    let plan_height_used = u16::try_from(visible_plan_lines.len()).unwrap_or(0);

    let progress_lines = app.progress_indicator().render(
        theme.info,
        theme.warning,
        theme.text_secondary,
        theme.muted,
        app.content_padding(),
    );

    let mut lines = progress_lines;
    lines.extend(renderer.committed_lines().to_vec());
    lines.extend(live_history_lines(app, renderer, live_area.width));
    let available = live_area.height.saturating_sub(plan_height_used) as usize;
    if lines.len() > available {
        lines.drain(..lines.len() - available);
    }
    let content_height = u16::try_from(lines.len()).unwrap_or(live_area.height).min(live_area.height);
    let content_area =
        Rect { y: live_area.bottom().saturating_sub(content_height), height: content_height, ..live_area };
    frame.render_widget(Paragraph::new(Text::from(lines)), content_area);

    if plan_height_used > 0 {
        let plan_area =
            Rect { y: content_area.y.saturating_sub(plan_height_used), height: plan_height_used, ..live_area };
        frame.render_widget(Paragraph::new(Text::from(visible_plan_lines)), plan_area);
    }

    render_composer(frame, &composer_layout, &overlay_lines, &prompt_search_lines, app, composer_area);
    render_status_line(frame, app, status_area, &theme, &settings);
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
            HistoryItem::Tool { title, status, diff, raw_input, display_value, sub_agents } => {
                let mut lines =
                    tool_lines(title, raw_input, display_value.as_deref(), status, spinner_tick, padding, &theme);
                if matches!(status, ToolStatus::Success)
                    && let Some(preview) = diff
                {
                    let rendered = render_diff(preview, content_width, &theme, self.highlighter());
                    lines.extend(indent_lines(rendered, padding));
                }
                if !sub_agents.is_empty() {
                    lines.push(Line::default());
                    lines.extend(sub_agent_tree_lines(sub_agents, spinner_tick, padding, &theme));
                }
                lines
            }
        }
    }
}

fn resolve_composer_height(requested_height: usize, area_height: u16, status_line_rows: u16) -> u16 {
    u16::try_from(requested_height).unwrap_or(u16::MAX).max(1).min(area_height.saturating_sub(status_line_rows))
}

fn live_area_height(area: Rect, app: &mut App, renderer: &TranscriptRenderer) -> u16 {
    let theme = renderer.theme().clone();
    let settings = renderer.settings().clone();
    let status_line_rows = status_line_height(app, area.width, &settings, &theme);
    let composer_layout = app.composer().layout(area.width, &theme);
    let overlay_height = app.composer().overlay_lines(area.width, 6, &theme).len();
    let prompt_search_height = app.composer().prompt_search_lines(area.width, 12, &theme).len();
    let requested_composer_height =
        composer_layout.lines.len().saturating_add(overlay_height).saturating_add(prompt_search_height);
    let composer_height = resolve_composer_height(requested_composer_height, area.height, status_line_rows);
    let remaining = area.height.saturating_sub(composer_height).saturating_sub(status_line_rows);
    let plan_lines = {
        let padding = app.content_padding();
        render_plan_lines(app.plan_entries(), &theme, padding)
    };
    let plan_height = u16::try_from(plan_lines.len()).unwrap_or(0);
    let clipped_plan = plan_height.min(remaining.div_ceil(3));
    let progress_height = u16::try_from(app.progress_indicator().line_count()).unwrap_or(0);
    remaining.saturating_sub(clipped_plan).saturating_sub(progress_height)
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

const MAX_TOOL_ARG_LENGTH: usize = 200;

fn tool_lines(
    title: &str,
    raw_input: &str,
    display_value: Option<&str>,
    status: &ToolStatus,
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (glyph, glyph_style, suffix) = match status {
        ToolStatus::Running => (spinner_frame(spinner_tick), Style::new().fg(theme.info), None),
        ToolStatus::Success => ("✓", Style::new().fg(theme.success), None),
        ToolStatus::Error(cause) => ("✗", Style::new().fg(theme.error), Some(format!(" {cause}"))),
    };
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        Span::styled(glyph.to_string(), glyph_style),
        Span::raw(" "),
        Span::styled(title.to_string(), Style::new().fg(theme.text_primary)),
    ];

    let display_text = display_value.filter(|v| !v.is_empty()).map_or_else(
        || match status {
            ToolStatus::Running => String::new(),
            _ => format_arguments(raw_input),
        },
        |v| format!(" ({v})"),
    );
    spans.push(Span::styled(display_text, Style::new().fg(theme.muted)));

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

fn sub_agent_tree_lines(
    sub_agents: &[SubAgentHistoryItem],
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let pad = " ".repeat(padding);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, agent) in sub_agents.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(format!("{pad}  ")));
        }

        // Agent header: spinner/check + agent name
        let (glyph, glyph_style) = if agent.done {
            ("✓", Style::new().fg(theme.success))
        } else {
            (spinner_frame(spinner_tick), Style::new().fg(theme.info))
        };
        let mut agent_line = Line::raw(format!("{pad}  "));
        agent_line.push_span(Span::styled(glyph, glyph_style));
        agent_line.push_span(Span::raw(format!(" {}", agent.agent_name)));
        lines.push(agent_line);

        let hidden_count = agent.tools.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);
        if hidden_count > 0 {
            lines.push(Line::styled(
                format!("{pad}  … {hidden_count} earlier tool calls"),
                Style::new().fg(theme.muted),
            ));
        }

        let visible: Vec<_> = agent.tools.iter().skip(hidden_count).collect();
        let muted = Style::new().fg(theme.muted);

        for (j, tool) in visible.iter().enumerate() {
            let is_last = j == visible.len() - 1;
            let (head_str, _tail_str) = if is_last { ("  └─ ", "     ") } else { ("  ├─ ", "  │  ") };

            let (glyph, glyph_style) = match &tool.status {
                ToolStatus::Running => (spinner_frame(spinner_tick), Style::new().fg(theme.info)),
                ToolStatus::Success => ("✓", Style::new().fg(theme.success)),
                ToolStatus::Error(_) => ("✗", Style::new().fg(theme.error)),
            };

            let display = tool.display_value.as_deref().filter(|v| !v.is_empty()).map_or_else(
                || match &tool.status {
                    ToolStatus::Running => String::new(),
                    _ => format_arguments(&tool.arguments),
                },
                |v| format!(" ({v})"),
            );

            let mut line = Line::from(vec![
                Span::raw(format!("{pad}{head_str}")),
                Span::styled(glyph, glyph_style),
                Span::raw(format!(" {}", tool.name)),
                Span::styled(display, muted),
            ]);

            if let ToolStatus::Error(msg) = &tool.status {
                line.push_span(Span::raw(" "));
                line.push_span(Span::styled(msg.clone(), Style::new().fg(theme.error)));
            }

            lines.push(line);
        }
    }

    lines
}

fn render_composer(
    frame: &mut Frame,
    layout: &crate::composer::ComposerLayout,
    overlay_lines: &[Line<'static>],
    prompt_search_lines: &[Line<'static>],
    app: &mut App,
    area: Rect,
) {
    let mut lines: Vec<Line<'static>> = prompt_search_lines.to_vec();
    lines.extend(layout.lines.clone());
    lines.extend_from_slice(overlay_lines);
    let skip = lines.len().saturating_sub(usize::from(area.height));
    let visible: Vec<Line<'static>> = lines.into_iter().skip(skip).collect();
    frame.render_widget(Paragraph::new(Text::from(visible)), area);

    let cursor_offset_y = if skip < prompt_search_lines.len() { prompt_search_lines.len() - skip } else { 0 };
    let x = area.x.saturating_add(layout.cursor.x);
    let y = area.y.saturating_add(u16::try_from(layout.cursor.y as usize + cursor_offset_y).unwrap_or(u16::MAX));
    if x < area.right() && y < area.bottom() {
        frame.set_cursor_position(Position::new(x, y));
    }

    if !prompt_search_lines.is_empty() {
        let ps_height = u16::try_from(prompt_search_lines.len()).unwrap_or(area.height);
        let ps_area = Rect { y: area.y, height: ps_height.min(area.height), ..area };
        app.set_surface_rect(ps_area);
    } else if !overlay_lines.is_empty() {
        let overlay_height = u16::try_from(overlay_lines.len()).unwrap_or(area.height);
        let overlay_area = Rect {
            y: area.y.saturating_add(u16::try_from(prompt_search_lines.len() + layout.lines.len()).unwrap_or(0)),
            height: overlay_height.min(area.height),
            ..area
        };
        app.set_surface_rect(overlay_area);
    } else if app.composer().has_prompt_search() || app.composer().has_overlay() {
        app.set_surface_rect(area);
    }
}

fn status_line_height(app: &App, width: u16, settings: &ResolvedStatusLineSettings, theme: &Theme) -> u16 {
    let width = width as usize;
    if width == 0 {
        return 1;
    }
    let left = render_left_section(app, settings, theme);
    let left_width = line_display_width(&left);
    let right = if app.exit_confirmation_active() {
        vec![Span::styled("Ctrl-C again to exit", Style::new().fg(theme.warning))]
    } else {
        render_right_section(app, settings, theme)
    };
    let right_width = line_display_width(&right);

    if right.is_empty() {
        return 1;
    }
    if left_width + 1 + right_width <= width {
        return 1;
    }
    2
}

fn render_status_line(frame: &mut Frame, app: &App, area: Rect, theme: &Theme, settings: &ResolvedStatusLineSettings) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }

    let left = render_left_section(app, settings, theme);
    let left_width = line_display_width(&left);

    let right = if app.exit_confirmation_active() {
        vec![Span::styled("Ctrl-C again to exit", Style::new().fg(theme.warning))]
    } else {
        render_right_section(app, settings, theme)
    };
    let right_width = line_display_width(&right);

    if right.is_empty() {
        let truncated = truncate_spans(&left, width);
        frame.render_widget(Paragraph::new(Line::from(truncated)), area);
    } else if left_width + 1 + right_width <= width {
        let mut spans = left.clone();
        let padding = width.saturating_sub(left_width + right_width);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.extend(right);
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    } else {
        let [left_row, right_row] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        let truncated_left = truncate_spans(&left, width);
        frame.render_widget(Paragraph::new(Line::from(truncated_left)), left_row);
        let right_spans = truncate_spans(&right, width);
        frame.render_widget(Paragraph::new(Line::from(right_spans)), right_row);
    }
}

fn render_left_section(app: &App, settings: &ResolvedStatusLineSettings, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ".repeat(app.content_padding()))];
    spans.extend(join_segments(app, &settings.left, &settings.separator, theme));
    spans
}

fn render_right_section(app: &App, settings: &ResolvedStatusLineSettings, theme: &Theme) -> Vec<Span<'static>> {
    join_segments(app, &settings.right, &settings.separator, theme)
}

fn join_segments(
    app: &App,
    segments: &[StatusLineSegmentConfig],
    separator: &str,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut first = true;
    for segment in segments {
        let segment_spans = render_segment(segment, app, theme);
        if segment_spans.is_empty() {
            continue;
        }
        if !first {
            spans.push(Span::styled(separator.to_string(), Style::new().fg(theme.text_secondary)));
        }
        spans.extend(segment_spans);
        first = false;
    }
    spans
}

fn render_segment(segment: &StatusLineSegmentConfig, app: &App, theme: &Theme) -> Vec<Span<'static>> {
    match segment {
        StatusLineSegmentConfig::Cwd { max_width } => {
            let dir = truncate_text(&app.workspace_status().display_dir, *max_width);
            vec![Span::styled(dir, Style::new().fg(theme.text_secondary))]
        }
        StatusLineSegmentConfig::GitRef => {
            let Some(git_ref) = app.workspace_status().git_ref.as_deref() else {
                return Vec::new();
            };
            vec![Span::styled(git_ref.to_string(), Style::new().fg(theme.success))]
        }
        StatusLineSegmentConfig::Agent => {
            vec![Span::styled(app.agent_name().to_string(), Style::new().fg(theme.info))]
        }
        StatusLineSegmentConfig::Mode => {
            let Some(mode_text) = extract_mode_display(app.config_options()) else {
                return Vec::new();
            };
            vec![Span::styled(mode_text, Style::new().fg(theme.text_secondary))]
        }
        StatusLineSegmentConfig::Model { max_width } => {
            let Some(model_summary) = extract_model_display(app.config_options()) else {
                return Vec::new();
            };
            let truncated = truncate_text(&model_summary, *max_width);
            vec![Span::styled(truncated, Style::new().fg(theme.success))]
        }
        StatusLineSegmentConfig::Reasoning => {
            let reasoning_levels = extract_reasoning_levels(app.config_options());
            if reasoning_levels.is_empty() {
                return Vec::new();
            }
            let reasoning_effort = extract_reasoning_effort(app.config_options());
            let color = reasoning_color(reasoning_effort, &reasoning_levels, theme);
            vec![Span::styled(reasoning_bar(reasoning_effort, &reasoning_levels), Style::new().fg(color))]
        }
        StatusLineSegmentConfig::Context => {
            let Some(usage) = app.context_usage() else {
                return Vec::new();
            };
            let color = context_color(usage, theme);
            vec![Span::styled(context_bar(usage), Style::new().fg(color))]
        }
        StatusLineSegmentConfig::ServerHealth => {
            if app.waiting_for_response() || app.unhealthy_server_count() == 0 {
                return Vec::new();
            }
            let count = app.unhealthy_server_count();
            let msg = if count == 1 { "1 server needs auth".to_string() } else { format!("{count} servers unhealthy") };
            vec![Span::styled(msg, Style::new().fg(theme.warning))]
        }
        StatusLineSegmentConfig::Text { value, style } => {
            let color = style.map_or(theme.text_secondary, |s| semantic_color(s, theme));
            vec![Span::styled(value.clone(), Style::new().fg(color))]
        }
    }
}

fn semantic_color(style: StatusLineStyle, theme: &Theme) -> ratatui::style::Color {
    match style {
        StatusLineStyle::Primary => theme.text_primary,
        StatusLineStyle::Secondary => theme.text_secondary,
        StatusLineStyle::Muted => theme.muted,
        StatusLineStyle::Info => theme.info,
        StatusLineStyle::Success => theme.success,
        StatusLineStyle::Warning => theme.warning,
        StatusLineStyle::Error => theme.error,
    }
}

fn line_display_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn truncate_spans(spans: &[Span<'static>], max_width: usize) -> Vec<Span<'static>> {
    let display_width: usize = spans.iter().map(Span::width).sum();
    if display_width <= max_width {
        return spans.to_vec();
    }
    let ellipsis = "…";
    let ellipsis_width = 1;
    if max_width < ellipsis_width {
        return Vec::new();
    }
    let budget = max_width - ellipsis_width;
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut remaining = budget;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let text = &span.content;
        let style = span.style;
        let mut byte_end = 0;
        let mut col = 0;
        for (i, ch) in text.char_indices() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + cw > remaining {
                break;
            }
            col += cw;
            byte_end = i + ch.len_utf8();
        }
        if byte_end > 0 {
            result.push(Span::styled(text[..byte_end].to_string(), style));
        }
        remaining -= col;
    }
    result.push(Span::raw(ellipsis));
    result
}

fn truncate_text(text: &str, max_width: Option<u16>) -> String {
    let Some(max_width) = max_width.map(usize::from) else {
        return text.to_string();
    };
    if text.width() <= max_width {
        return text.to_string();
    }
    let mut result = String::new();
    let mut current_width = 0;
    for ch in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + char_width > max_width.saturating_sub(1) {
            result.push('…');
            break;
        }
        result.push(ch);
        current_width += char_width;
    }
    result
}

fn slot_bar(filled: usize, total: usize) -> String {
    let slots: String = (0..total).map(|i| if i < filled { '■' } else { '·' }).collect();
    format!("[{slots}]")
}

fn context_bar(usage: ContextUsageDisplay) -> String {
    const TOTAL: u32 = 3;
    let filled = (usage.used_tokens.saturating_mul(TOTAL) + usage.limit_tokens / 2) / usage.limit_tokens.max(1);
    let filled = (filled as usize).min(TOTAL as usize);
    format!(
        "ctx {} {} / {}",
        slot_bar(filled, TOTAL as usize),
        format_tokens(usage.used_tokens),
        format_tokens(usage.limit_tokens)
    )
}

fn context_color(usage: ContextUsageDisplay, theme: &Theme) -> ratatui::style::Color {
    let used_pct = usage.used_ratio() * 100.0;
    if used_pct >= 86.0 {
        theme.error
    } else if used_pct >= 71.0 {
        theme.warning
    } else {
        theme.text_secondary
    }
}

fn format_tokens(n: u32) -> String {
    match n {
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => format_with_unit(f64::from(n) / 1_000.0, "k"),
        n => format_with_unit(f64::from(n) / 1_000_000.0, "M"),
    }
}

fn format_with_unit(value: f64, unit: &str) -> String {
    let rounded_one = (value * 10.0).round() / 10.0;
    if (rounded_one - rounded_one.trunc()).abs() < f64::EPSILON {
        format!("{rounded_one:.0}{unit}")
    } else {
        format!("{rounded_one:.1}{unit}")
    }
}

fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> String {
    let label = effort.map_or("none", |e| e.as_str());
    format!("{label} {}", slot_bar(filled_slots(effort, levels), levels.len()))
}

fn reasoning_color(
    effort: Option<ReasoningEffort>,
    levels: &[ReasoningEffort],
    theme: &Theme,
) -> ratatui::style::Color {
    let filled = filled_slots(effort, levels);
    let total_levels = levels.len();
    if total_levels == 0 {
        theme.text_secondary
    } else if effort == Some(ReasoningEffort::Max) {
        theme.warning
    } else if filled * 3 <= total_levels {
        theme.text_secondary
    } else if filled * 3 <= total_levels * 2 {
        theme.info
    } else {
        theme.success
    }
}

fn filled_slots(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> usize {
    effort.map_or(0, |effort| levels.iter().filter(|&&level| level <= effort).count())
}

fn extract_reasoning_levels(config_options: &[SessionConfigOption]) -> Vec<ReasoningEffort> {
    let Some(option) = config_options.iter().find(|o| o.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())
    else {
        return Vec::new();
    };
    let SessionConfigKind::Select(ref select) = option.kind else {
        return Vec::new();
    };
    let SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return Vec::new();
    };
    options.iter().filter_map(|o| o.value.0.as_ref().parse().ok()).collect()
}

fn extract_reasoning_effort(config_options: &[SessionConfigOption]) -> Option<ReasoningEffort> {
    let option =
        config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())?;
    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };
    ReasoningEffort::parse(&select.current_value.0).unwrap_or(None)
}

fn extract_mode_display(config_options: &[SessionConfigOption]) -> Option<String> {
    extract_select_display(config_options, ConfigOptionId::Mode)
}

fn extract_model_display(config_options: &[SessionConfigOption]) -> Option<String> {
    let option = config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::Model.as_str())?;
    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };
    let options = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options,
        SessionConfigSelectOptions::Grouped(_) => {
            return extract_select_display(config_options, ConfigOptionId::Model);
        }
        _ => return None,
    };
    let current = select.current_value.0.as_ref();
    if current.contains(',') {
        let names: Vec<&str> = current
            .split(',')
            .filter_map(|part| {
                let trimmed = part.trim();
                options.iter().find(|option| option.value.0.as_ref() == trimmed).map(|option| option.name.as_str())
            })
            .collect();
        if names.is_empty() { None } else { Some(names.join(" + ")) }
    } else {
        extract_select_display(config_options, ConfigOptionId::Model)
    }
}

fn extract_select_display(config_options: &[SessionConfigOption], id: ConfigOptionId) -> Option<String> {
    let option = config_options.iter().find(|option| option.id.0.as_ref() == id.as_str())?;
    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };
    option_display_name(&select.options, &select.current_value)
}

fn option_display_name(
    options: &SessionConfigSelectOptions,
    current_value: &acp::SessionConfigValueId,
) -> Option<String> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().find(|option| &option.value == current_value).map(|option| option.name.clone())
        }
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .find(|option| &option.value == current_value)
            .map(|option| option.name.clone()),
        _ => None,
    }
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

fn format_arguments(arguments: &str) -> String {
    let mut formatted = format!(" {arguments}");
    let char_count = formatted.chars().count();
    if char_count > MAX_TOOL_ARG_LENGTH {
        let ellipsis = "…";
        let max_chars = MAX_TOOL_ARG_LENGTH.saturating_sub(ellipsis.chars().count());
        formatted = formatted.chars().take(max_chars).collect();
        formatted.push_str(ellipsis);
    }
    formatted
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
