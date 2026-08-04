use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::app::{App, HistoryItem, HistoryKind};
use crate::components::diff::render_diff;
use crate::components::generation::Generation;
use crate::components::markdown::render_markdown;
use crate::components::syntax::SyntaxHighlighter;
use crate::components::theme::Theme;
use crate::components::widgets::RowsView;
use crate::components::wrap::{as_u16, truncate_to_width, wrap_line, wrap_text};
use crate::conversation::plan_view::PlanView;
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressIndicatorView, spinner_frame};
use crate::conversation::status_line::StatusLine;
use crate::conversation::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, SubAgentState, ToolStatus};
use crate::session::terminal::INLINE_SCROLLBACK_RESERVE;
use crate::settings::UiSettings;
use crate::surfaces::composer::{ComposerBodyView, ComposerLayout};
use agent_client_protocol::schema::PlanEntry;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr;

/// Most rows the completion list and history search may claim.
const COMPLETION_MAX_ROWS: usize = 6;
const PROMPT_SEARCH_MAX_ROWS: usize = 12;
const MAX_TOOL_ARG_WIDTH: usize = 200;

/// Which half of the transcript is being rendered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Segment {
    /// Has left the live viewport for good.
    Committed,
    /// Still mutating, so its markdown is cached between frames.
    Live,
}

/// Shared rendering services, borrowed for the duration of one draw.
///
/// `theme_generation` lets a surface cache styled text across frames and
/// rebuild it only when the theme actually changes.
pub struct DrawContext<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a mut SyntaxHighlighter,
    pub theme_generation: Generation,
}

/// Owns everything drawing a frame needs: the theme, the syntax highlighter,
/// and the caches that keep streaming content stable between frames.
///
/// Methods destructure `self` rather than taking `&self.theme` through `&mut
/// self`, so the theme is borrowed alongside the highlighter instead of cloned
/// once per item per frame.
pub struct Renderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    theme_generation: Generation,
    pending_markdown_cache: PendingMarkdownCache,
    scrollback: Scrollback,
}

impl Renderer {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            theme: Theme::load(settings),
            highlighter: SyntaxHighlighter::new(),
            theme_generation: Generation::default(),
            pending_markdown_cache: PendingMarkdownCache::default(),
            scrollback: Scrollback::default(),
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.theme_generation.bump();
        self.highlighter.clear();
        self.pending_markdown_cache.clear();
    }

    /// Draws one frame, moving any scrollback that no longer fits into the
    /// terminal's own history above the inline viewport.
    pub fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error> {
        terminal.autoresize()?;
        let terminal_height = terminal.size()?.height;
        let area = terminal.get_frame().area();
        self.sync_transcript_generation(app.transcript_generation());
        let can_insert_history = terminal_height.saturating_sub(area.height) >= INLINE_SCROLLBACK_RESERVE;
        let commits_history = can_insert_history && !app.full_screen_active();

        if commits_history {
            let previous_kind = app.last_drained_kind();
            let committed = app.drain_finalized();
            let lines = self.lines(
                Segment::Committed,
                &committed,
                previous_kind,
                area.width,
                app.content_padding(),
                app.spinner_tick(),
            );
            self.scrollback.append(lines);
        }

        let layout = FrameLayout::new(area, app, self);

        // Scrollback leaves for the terminal's own history before the frame is
        // painted, so the viewport is drawn once, already in its final position.
        if commits_history {
            let overflow = self.scrollback.take_overflow(layout.committed_capacity());
            for chunk in overflow.chunks(usize::from(u16::MAX)) {
                let chunk = chunk.to_vec();
                terminal.insert_before(as_u16(chunk.len()), move |buffer| {
                    Paragraph::new(Text::from(chunk)).render(buffer.area, buffer);
                })?;
            }
        }

        terminal.draw(|frame| draw_frame(frame, app, self, &layout))?;
        Ok(())
    }

    /// Renders `items` back to back, inserting a blank line wherever the kind
    /// of content changes so speakers and tool calls stay visually separated.
    fn lines(
        &mut self,
        segment: Segment,
        items: &[HistoryItem<'_>],
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
            match item {
                // Only the live tail is worth caching: a committed segment is
                // rendered once and never asked for again.
                HistoryItem::Text(text) if segment == Segment::Live => {
                    lines.extend(self.cached_markdown(text, width, padding).iter().cloned());
                }
                _ => lines.extend(self.item_lines(item, width, padding, spinner_tick)),
            }
            previous = Some(kind);
        }
        if segment == Segment::Live {
            self.pending_markdown_cache.end_frame();
        }
        lines
    }

    /// The rendering services a full-screen surface or overlay draws with.
    fn context(&mut self) -> DrawContext<'_> {
        DrawContext { theme: &self.theme, highlighter: &mut self.highlighter, theme_generation: self.theme_generation }
    }

    /// Drops everything cached for a conversation that has been swapped out.
    fn sync_transcript_generation(&mut self, generation: Generation) {
        if self.scrollback.sync(generation) {
            self.pending_markdown_cache.clear();
        }
    }

    fn cached_markdown(&mut self, text: &str, width: u16, padding: usize) -> Rc<[Line<'static>]> {
        let Self { theme, highlighter, pending_markdown_cache, .. } = self;
        let content_width = content_width(width, padding);
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        pending_markdown_cache.get_or_insert_with((hasher.finish(), content_width), || {
            indent_lines(render_markdown(text, content_width, theme, highlighter), padding)
        })
    }

    fn item_lines(
        &mut self,
        item: &HistoryItem<'_>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let Self { theme, highlighter, .. } = self;
        let content_width = content_width(width, padding);
        match item {
            HistoryItem::User(text) => user_block_lines(text, width, padding, theme),
            HistoryItem::Text(text) => indent_lines(render_markdown(text, content_width, theme, highlighter), padding),
            HistoryItem::Thought(text) => indent_lines(
                wrap_line(
                    Line::styled(
                        text.to_string(),
                        Style::new().fg(theme.blockquote).add_modifier(Modifier::ITALIC | Modifier::DIM),
                    ),
                    content_width,
                ),
                padding,
            ),
            HistoryItem::Tool { title, status, diff, raw_input, display_value, sub_agents } => {
                let mut lines =
                    vec![tool_line(title, raw_input, display_value.as_deref(), status, spinner_tick, padding, theme)];
                if matches!(status.as_ref(), ToolStatus::Success)
                    && let Some(preview) = diff
                {
                    let rendered = render_diff(preview, content_width, theme, highlighter);
                    lines.extend(indent_lines(rendered, padding));
                }
                if !sub_agents.is_empty() {
                    lines.push(Line::default());
                    lines.extend(sub_agent_tree_lines(sub_agents, spinner_tick, padding, theme));
                }
                lines
            }
        }
    }
}

/// The rendered lines that have left the live viewport. What still fits on
/// screen is drawn; anything older is handed to the terminal's own history.
#[derive(Default)]
struct Scrollback {
    lines: Vec<Line<'static>>,
    generation: Generation,
}

impl Scrollback {
    fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    fn append(&mut self, lines: Vec<Line<'static>>) {
        self.lines.extend(lines);
    }

    /// Everything beyond the newest `visible` rows, for handing to the terminal.
    fn take_overflow(&mut self, visible: usize) -> Vec<Line<'static>> {
        let overflow = self.lines.len().saturating_sub(visible);
        self.lines.drain(..overflow).collect()
    }

    /// Drops the scrollback when the conversation it belongs to is swapped out,
    /// reporting whether that happened.
    fn sync(&mut self, generation: Generation) -> bool {
        if self.generation == generation {
            return false;
        }
        self.lines.clear();
        self.generation = generation;
        true
    }
}

/// Rendered markdown for the streaming tail, keyed by content hash and width.
///
/// Each frame's entries are collected separately and become the whole cache
/// when the frame ends, so a message that grows by one chunk per frame reuses
/// its previous render without accumulating an entry per intermediate prefix.
#[derive(Default)]
struct PendingMarkdownCache {
    current: HashMap<(u64, u16), Rc<[Line<'static>]>>,
    frame: HashMap<(u64, u16), Rc<[Line<'static>]>>,
}

impl PendingMarkdownCache {
    fn get_or_insert_with(
        &mut self,
        key: (u64, u16),
        build: impl FnOnce() -> Vec<Line<'static>>,
    ) -> Rc<[Line<'static>]> {
        let lines = self.frame.remove(&key).or_else(|| self.current.remove(&key)).unwrap_or_else(|| Rc::from(build()));
        self.frame.insert(key, Rc::clone(&lines));
        lines
    }

    /// Drops every entry the frame that just rendered did not use.
    fn end_frame(&mut self) {
        self.current = std::mem::take(&mut self.frame);
    }

    fn clear(&mut self) {
        self.current.clear();
        self.frame.clear();
    }
}

/// Everything measured once per frame, before anything is drawn.
struct FrameLayout {
    composer_layout: ComposerLayout,
    completion_rows: u16,
    prompt_search_rows: u16,
    composer_height: u16,
    status_line: StatusLine,
    status_line_rows: u16,
    plan_entries: Vec<PlanEntry>,
    plan_height: u16,
    progress_height: u16,
    live_lines: Vec<Line<'static>>,
    /// Columns of blank gutter each side of the conversation content.
    content_padding: u16,
    /// Rows the conversation gets once the plan, composer, and status line are
    /// placed — what [`FrameLayout::split`]'s `Fill` resolves to.
    transcript_height: u16,
}

impl FrameLayout {
    fn new(area: Rect, app: &mut App, renderer: &mut Renderer) -> Self {
        let status_line = StatusLine::new(&app.status_line_model(), renderer.theme());
        let status_line_rows = status_line.height(area.width);
        let composer_layout = app.composer_mut().layout(area.width, renderer.theme());
        let completion_rows =
            as_u16(app.composer_mut().completion().map_or(0, |overlay| overlay.row_count(COMPLETION_MAX_ROWS)));
        let prompt_search_rows =
            as_u16(app.composer_mut().prompt_search().map_or(0, |picker| picker.height(PROMPT_SEARCH_MAX_ROWS)));
        let requested_composer_height =
            as_u16(composer_layout.lines.len()).saturating_add(completion_rows).saturating_add(prompt_search_rows);
        let composer_height = requested_composer_height.max(1).min(area.height.saturating_sub(status_line_rows));

        let remaining = area.height.saturating_sub(composer_height).saturating_sub(status_line_rows);
        let plan_entries = app.plan_entries();
        // The plan never takes more than a third of what is left, so a long plan
        // cannot squeeze out the conversation.
        let plan_height =
            as_u16(PlanView::new(&plan_entries, renderer.theme()).line_count()).min(remaining.div_ceil(3));
        let progress_height =
            ProgressIndicatorView::new(app.progress_indicator(), renderer.theme(), app.spinner_tick()).height();
        let live_lines =
            if app.full_screen_active() { Vec::new() } else { live_history_lines(app, renderer, area.width) };

        Self {
            composer_layout,
            completion_rows,
            prompt_search_rows,
            composer_height,
            status_line,
            status_line_rows,
            plan_entries,
            plan_height,
            progress_height,
            live_lines,
            content_padding: as_u16(app.content_padding()),
            transcript_height: remaining.saturating_sub(plan_height),
        }
    }

    /// Splits the frame into the four stacked bands it always has. The plan and
    /// the conversation share whatever the composer and status line leave.
    fn split(&self, area: Rect) -> [Rect; 4] {
        Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(self.plan_height),
            Constraint::Length(self.composer_height),
            Constraint::Length(self.status_line_rows),
        ])
        .areas(area)
    }

    /// `area` inset by the content gutter, for the widgets that draw inside it.
    fn indent(&self, area: Rect) -> Rect {
        area.inner(Margin::new(self.content_padding, 0))
    }

    /// Committed scrollback rows that still fit on screen; anything older is
    /// handed to the terminal's own history.
    fn committed_capacity(&self) -> usize {
        usize::from(self.transcript_height)
            .saturating_sub(self.live_lines.len())
            .saturating_sub(usize::from(self.progress_height))
    }
}

fn draw_frame(frame: &mut Frame, app: &mut App, renderer: &mut Renderer, layout: &FrameLayout) {
    let [transcript_area, plan_area, composer_area, status_area] = layout.split(frame.area());
    let buf = frame.buffer_mut();

    PlanView::new(&layout.plan_entries, renderer.theme()).render(layout.indent(plan_area), buf);
    draw_transcript(
        layout,
        renderer.scrollback.lines(),
        transcript_area,
        buf,
        app.progress_indicator(),
        renderer.theme(),
        app.spinner_tick(),
    );

    let cursor = render_composer(layout, app, composer_area, buf, renderer.theme());
    layout.status_line.render(status_area, buf);

    let layer_cursor = app.render_layer(frame.area(), frame.buffer_mut(), &mut renderer.context());
    if let Some(cursor) = layer_cursor.or(cursor) {
        frame.set_cursor_position(cursor);
    }
}

/// Stacks committed scrollback, the streaming tail, and the progress indicator
/// against the bottom of `area`, trimming the oldest rows first.
fn draw_transcript(
    layout: &FrameLayout,
    committed: &[Line<'static>],
    area: Rect,
    buf: &mut Buffer,
    progress_indicator: &ProgressIndicator,
    theme: &Theme,
    tick: usize,
) {
    let mut remaining = usize::from(area.height);
    let progress_height = usize::from(layout.progress_height).min(remaining);
    remaining = remaining.saturating_sub(progress_height);
    let live = tail(&layout.live_lines, &mut remaining);
    let committed = tail(committed, &mut remaining);

    let [committed_area, live_area, progress_area] = Layout::vertical([
        Constraint::Length(as_u16(committed.len())),
        Constraint::Length(as_u16(live.len())),
        Constraint::Length(as_u16(progress_height)),
    ])
    .areas(area);
    RowsView::new(committed).render(committed_area, buf);
    RowsView::new(live).render(live_area, buf);
    ProgressIndicatorView::new(progress_indicator, theme, tick).render(layout.indent(progress_area), buf);
}

/// The last `remaining` lines of `lines`, decrementing `remaining` by how many
/// were taken.
fn tail<'a>(lines: &'a [Line<'static>], remaining: &mut usize) -> &'a [Line<'static>] {
    let taken = lines.len().min(*remaining);
    *remaining -= taken;
    &lines[lines.len() - taken..]
}

fn live_history_lines(app: &App, renderer: &mut Renderer, width: u16) -> Vec<Line<'static>> {
    renderer.lines(
        Segment::Live,
        &app.pending_items(),
        app.last_drained_kind(),
        width,
        app.content_padding(),
        app.spinner_tick(),
    )
}

/// Lays out the composer: history search on top, then the text area, then the
/// completion list. Returns where the text cursor should sit.
fn render_composer(
    layout: &FrameLayout,
    app: &mut App,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
) -> Option<Position> {
    let [search_area, body_area, completion_area] = Layout::vertical([
        Constraint::Length(layout.prompt_search_rows.min(area.height)),
        Constraint::Fill(1),
        Constraint::Length(layout.completion_rows),
    ])
    .areas(area);

    if search_area.height > 0
        && let Some(picker) = app.composer_mut().prompt_search()
    {
        picker.render(search_area, buf, theme);
    }
    if completion_area.height > 0
        && let Some(overlay) = app.composer_mut().completion()
    {
        let (view, selection) = overlay.view(theme);
        StatefulWidget::render(view, completion_area, buf, selection);
    }

    let skip = layout.composer_layout.lines.len().saturating_sub(usize::from(body_area.height));
    let body = ComposerBodyView::new(&layout.composer_layout, skip);
    let cursor = body.cursor_position(body_area);
    body.render(body_area, buf);
    cursor
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

fn content_width(width: u16, padding: usize) -> u16 {
    width.saturating_sub(u16::try_from(padding.saturating_mul(2)).unwrap_or(u16::MAX)).max(1)
}

/// The user's own message, drawn as a full-width tinted block.
fn user_block_lines(text: &str, width: u16, padding: usize, theme: &Theme) -> Vec<Line<'static>> {
    let block_style = Style::new().bg(theme.sidebar_bg).fg(theme.text_primary);
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

/// Status marker for a tool call: a spinner while running, then a verdict.
fn status_glyph(status: &ToolStatus, spinner_tick: usize, theme: &Theme) -> Span<'static> {
    let (glyph, color) = match status {
        ToolStatus::Running => (spinner_frame(spinner_tick), theme.info),
        ToolStatus::Success => ("✓", theme.success),
        ToolStatus::Error(_) => ("✗", theme.error),
    };
    Span::styled(glyph, Style::new().fg(color))
}

/// The trailing detail on a tool line: the agent's own summary when it supplied
/// one, otherwise the raw arguments. A running tool shows nothing until it has
/// something to report.
fn tool_detail(display_value: Option<&str>, raw_input: &str, status: &ToolStatus) -> String {
    match display_value.filter(|value| !value.is_empty()) {
        Some(value) => format!(" ({value})"),
        None if matches!(status, ToolStatus::Running) => String::new(),
        None => format!(" {}", truncate_to_width(raw_input, MAX_TOOL_ARG_WIDTH)),
    }
}

fn tool_line(
    title: &str,
    raw_input: &str,
    display_value: Option<&str>,
    status: &ToolStatus,
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        status_glyph(status, spinner_tick, theme),
        Span::raw(" "),
        Span::styled(title.to_string(), Style::new().fg(theme.text_primary)),
        Span::styled(tool_detail(display_value, raw_input, status), Style::new().fg(theme.muted)),
    ];
    if let ToolStatus::Error(cause) = status {
        spans.push(Span::styled(format!(" {cause}"), Style::new().fg(theme.error)));
    }
    Line::from(spans)
}

/// Tree of sub-agents beneath a spawning tool, each with its recent tool calls.
fn sub_agent_tree_lines(
    sub_agents: &[SubAgentState],
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let pad = " ".repeat(padding);
    let muted = Style::new().fg(theme.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (index, agent) in sub_agents.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(format!("{pad}  ")));
        }

        let done = if agent.done { ToolStatus::Success } else { ToolStatus::Running };
        lines.push(Line::from(vec![
            Span::raw(format!("{pad}  ")),
            status_glyph(&done, spinner_tick, theme),
            Span::raw(format!(" {}", agent.agent_name)),
        ]));

        // Only the most recent calls are shown; older ones collapse into a count.
        let hidden = agent.tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);
        if hidden > 0 {
            lines.push(Line::styled(format!("{pad}  … {hidden} earlier tool calls"), muted));
        }

        let visible: Vec<_> = agent.tool_calls.iter().skip(hidden).collect();
        for (index, tool) in visible.iter().enumerate() {
            let branch = if index + 1 == visible.len() { "  └─ " } else { "  ├─ " };
            let mut line = Line::from(vec![
                Span::raw(format!("{pad}{branch}")),
                status_glyph(&tool.status, spinner_tick, theme),
                Span::raw(format!(" {}", tool.name)),
                Span::styled(tool_detail(tool.display_value.as_deref(), &tool.arguments, &tool.status), muted),
            ]);
            if let ToolStatus::Error(message) = &tool.status {
                line.push_span(Span::styled(format!(" {message}"), Style::new().fg(theme.error)));
            }
            lines.push(line);
        }
    }

    lines
}
