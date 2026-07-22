use crate::INLINE_SCROLLBACK_RESERVE;
use crate::app::{App, HistoryItem, HistoryKind, SubAgentHistoryItem};
use crate::diff::render_diff;
use crate::markdown::render_markdown;
use crate::plan_view::render_plan_lines;
use crate::presentation::TranscriptRenderer;
use crate::progress_indicator::spinner_frame;
use crate::session_config_view::SessionConfigView;
use crate::settings::{ContextUsageDisplay, ResolvedStatusLineSettings, StatusLineSegmentConfig, StatusLineStyle};
use crate::theme::Theme;
use crate::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, ToolStatus};
use crate::wrap::{truncate_spans, truncate_to_width, wrap_line, wrap_text};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::SessionConfigOption;
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

    let layout = FrameLayout::new(area, app, renderer);
    let pending_line_count = if full_screen_active { 0 } else { layout.live_lines.len() };
    let visible_history_lines = usize::from(layout.live_history_height);
    let committed_to_keep = visible_history_lines.saturating_sub(pending_line_count);
    let overflow = if can_insert_history && !full_screen_active {
        renderer.take_committed_overflow(committed_to_keep)
    } else {
        Vec::new()
    };

    terminal.draw(|frame| draw_frame(frame, app, renderer, &layout))?;

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

    terminal.draw(|frame| draw_frame(frame, app, renderer, &layout))?;
    Ok(())
}

pub fn draw(frame: &mut Frame, app: &mut App, renderer: &mut TranscriptRenderer) {
    let layout = FrameLayout::new(frame.area(), app, renderer);
    draw_frame(frame, app, renderer, &layout);
}

struct FrameLayout {
    composer_layout: crate::composer::ComposerLayout,
    overlay_lines: Vec<Line<'static>>,
    prompt_search_height: usize,
    composer_height: u16,
    status_line_rows: u16,
    plan_lines: Vec<Line<'static>>,
    plan_height: u16,
    progress_lines: Vec<Line<'static>>,
    live_lines: Vec<Line<'static>>,
    live_history_height: u16,
}

impl FrameLayout {
    fn new(area: Rect, app: &mut App, renderer: &mut TranscriptRenderer) -> Self {
        let theme = renderer.theme().clone();
        let settings = renderer.settings().clone();
        let status_line_rows = status_line_height(app, area.width, &settings, &theme);
        let composer_layout = app.composer().layout(area.width, &theme);
        let overlay_lines = app.composer().overlay_lines(area.width, 6, &theme);
        let prompt_search_height = app.composer().prompt_search_height(12);
        let requested_composer_height =
            composer_layout.lines.len().saturating_add(overlay_lines.len()).saturating_add(prompt_search_height);
        let composer_height = resolve_composer_height(requested_composer_height, area.height, status_line_rows);
        let remaining = area.height.saturating_sub(composer_height).saturating_sub(status_line_rows);
        let padding = app.content_padding();
        let plan_lines = render_plan_lines(app.plan_entries(), &theme, padding);
        let plan_height = u16::try_from(plan_lines.len()).unwrap_or(u16::MAX).min(remaining.div_ceil(3));
        let progress_lines = app.progress_indicator().render(
            theme.info,
            theme.warning,
            theme.text_secondary,
            theme.muted,
            app.spinner_tick(),
            app.content_padding(),
        );
        let live_lines =
            if app.full_screen_active() { Vec::new() } else { live_history_lines(app, renderer, area.width) };
        let live_history_height = remaining
            .saturating_sub(plan_height)
            .saturating_sub(u16::try_from(progress_lines.len()).unwrap_or(u16::MAX));
        Self {
            composer_layout,
            overlay_lines,
            prompt_search_height,
            composer_height,
            status_line_rows,
            plan_lines,
            plan_height,
            progress_lines,
            live_lines,
            live_history_height,
        }
    }
}

fn draw_frame(frame: &mut Frame, app: &mut App, renderer: &mut TranscriptRenderer, layout: &FrameLayout) {
    let theme = renderer.theme().clone();
    let settings = renderer.settings().clone();
    let [live_area, composer_area, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(layout.composer_height),
        Constraint::Length(layout.status_line_rows),
    ])
    .areas(frame.area());

    let mut lines = layout.progress_lines.clone();
    lines.extend(renderer.committed_lines().iter().cloned());
    lines.extend(layout.live_lines.iter().cloned());
    let available = live_area.height.saturating_sub(layout.plan_height) as usize;
    if lines.len() > available {
        lines.drain(..lines.len() - available);
    }
    let content_height = u16::try_from(lines.len()).unwrap_or(live_area.height).min(live_area.height);
    let [_, plan_area, content_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(layout.plan_height),
        Constraint::Length(content_height),
    ])
    .areas(live_area);
    if layout.plan_height > 0 {
        frame.render_widget(Paragraph::new(Text::from(layout.plan_lines.clone())), plan_area);
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), content_area);

    render_composer(
        frame,
        &layout.composer_layout,
        &layout.overlay_lines,
        layout.prompt_search_height,
        app,
        composer_area,
        &theme,
    );
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

    pub(crate) fn item_lines(
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

fn live_history_lines(app: &App, renderer: &mut TranscriptRenderer, width: u16) -> Vec<Line<'static>> {
    renderer.pending_history_lines(
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
    prompt_search_height: usize,
    app: &mut App,
    area: Rect,
    theme: &Theme,
) {
    let search_height = u16::try_from(prompt_search_height).unwrap_or(area.height).min(area.height);
    let [search_area, body_area] =
        Layout::vertical([Constraint::Length(search_height), Constraint::Min(0)]).areas(area);

    if search_height > 0 {
        app.composer_mut().render_prompt_search(search_area, frame.buffer_mut(), theme);
    }

    let mut lines = layout.lines.clone();
    lines.extend_from_slice(overlay_lines);
    let skip = lines.len().saturating_sub(usize::from(body_area.height));
    let visible: Vec<Line<'static>> = lines.into_iter().skip(skip).collect();
    frame.render_widget(Paragraph::new(Text::from(visible)), body_area);

    let x = body_area.x.saturating_add(layout.cursor.x);
    let cursor_row = layout.cursor.y as usize;
    let y = body_area.y.saturating_add(u16::try_from(cursor_row.saturating_sub(skip)).unwrap_or(u16::MAX));
    if cursor_row >= skip && x < body_area.right() && y < body_area.bottom() {
        frame.set_cursor_position(Position::new(x, y));
    }

    if search_height > 0 {
        app.set_surface_rect(search_area);
    } else if !overlay_lines.is_empty() {
        let overlay_height = u16::try_from(overlay_lines.len()).unwrap_or(body_area.height);
        let overlay_area = Rect {
            y: body_area.y.saturating_add(u16::try_from(layout.lines.len().saturating_sub(skip)).unwrap_or(0)),
            height: overlay_height.min(body_area.height),
            ..body_area
        };
        app.set_surface_rect(overlay_area);
    } else if app.composer().has_prompt_search() || app.composer().has_overlay() {
        app.set_surface_rect(area);
    }
}

fn status_line_height(app: &App, width: u16, settings: &ResolvedStatusLineSettings, theme: &Theme) -> u16 {
    if width == 0 {
        return 1;
    }
    let (left, right) = status_line_sections(app, settings, theme);
    if right.width() == 0 || left.width() + 1 + right.width() <= usize::from(width) { 1 } else { 2 }
}

fn render_status_line(frame: &mut Frame, app: &App, area: Rect, theme: &Theme, settings: &ResolvedStatusLineSettings) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (left, right) = status_line_sections(app, settings, theme);
    let width = usize::from(area.width);

    if right.width() == 0 || area.height == 1 && left.width() + 1 + right.width() > width {
        frame.render_widget(Paragraph::new(Line::from(truncate_spans(&left.spans, width))), area);
    } else if left.width() + 1 + right.width() <= width {
        frame.render_widget(Paragraph::new(left).alignment(ratatui::layout::Alignment::Left), area);
        frame.render_widget(Paragraph::new(right).alignment(ratatui::layout::Alignment::Right), area);
    } else {
        let [left_row, right_row] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(Paragraph::new(Line::from(truncate_spans(&left.spans, width))), left_row);
        frame.render_widget(Paragraph::new(Line::from(truncate_spans(&right.spans, width))), right_row);
    }
}

fn status_line_sections(
    app: &App,
    settings: &ResolvedStatusLineSettings,
    theme: &Theme,
) -> (Line<'static>, Line<'static>) {
    let left = Line::from(render_left_section(app, settings, theme));
    let right = if app.exit_confirmation_active() {
        Line::from(Span::styled("Ctrl-C again to exit", Style::new().fg(theme.warning)))
    } else {
        Line::from(render_right_section(app, settings, theme))
    };
    (left, right)
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
            let dir = max_width.map_or_else(
                || app.workspace_status().display_dir.clone(),
                |width| truncate_to_width(&app.workspace_status().display_dir, usize::from(width)),
            );
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
            let truncated = max_width
                .map_or_else(|| model_summary.clone(), |width| truncate_to_width(&model_summary, usize::from(width)));
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
    SessionConfigView::new(config_options).reasoning_levels()
}

fn extract_reasoning_effort(config_options: &[SessionConfigOption]) -> Option<ReasoningEffort> {
    SessionConfigView::new(config_options).reasoning_effort()
}

fn extract_mode_display(config_options: &[SessionConfigOption]) -> Option<String> {
    SessionConfigView::new(config_options).current_display_name(ConfigOptionId::Mode)
}

fn extract_model_display(config_options: &[SessionConfigOption]) -> Option<String> {
    SessionConfigView::new(config_options).current_display_name(ConfigOptionId::Model)
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
