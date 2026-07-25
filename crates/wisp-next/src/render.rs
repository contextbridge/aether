use crate::INLINE_SCROLLBACK_RESERVE;
use crate::app::App;
use crate::plan_view::PlanView;
use crate::presentation::Presenter;
use crate::status_line::{StatusLine, status_line_height};
use crate::theme::Theme;
use agent_client_protocol::schema::PlanEntry;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget};

/// Most rows the completion list and history search may claim.
const COMPLETION_MAX_ROWS: usize = 6;
const PROMPT_SEARCH_MAX_ROWS: usize = 12;

/// Draws one frame, moving any scrollback that no longer fits into the
/// terminal's own history above the inline viewport.
pub fn sync_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    presenter: &mut Presenter,
) -> Result<(), B::Error> {
    terminal.autoresize()?;
    let terminal_height = terminal.size()?.height;
    let area = terminal.get_frame().area();
    presenter.sync_transcript_generation(app.transcript_generation());
    let can_insert_history = terminal_height.saturating_sub(area.height) >= INLINE_SCROLLBACK_RESERVE;
    let commits_history = can_insert_history && !app.full_screen_active();

    if commits_history {
        let previous_kind = app.last_drained_kind();
        let committed = app.drain_finalized();
        let lines =
            presenter.history_lines(&committed, previous_kind, area.width, app.content_padding(), app.spinner_tick());
        presenter.append_committed_lines(lines);
    }

    let layout = FrameLayout::new(area, app, presenter);
    let overflow =
        if commits_history { presenter.take_committed_overflow(layout.committed_capacity()) } else { Vec::new() };

    terminal.draw(|frame| draw_frame(frame, app, presenter, &layout))?;

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

    terminal.draw(|frame| draw_frame(frame, app, presenter, &layout))?;
    Ok(())
}

/// Everything measured once per frame and reused by both draw passes.
struct FrameLayout {
    composer_layout: crate::composer::ComposerLayout,
    completion_rows: u16,
    prompt_search_rows: u16,
    composer_height: u16,
    status_line_rows: u16,
    plan_entries: Vec<PlanEntry>,
    plan_height: u16,
    progress_lines: Vec<Line<'static>>,
    live_lines: Vec<Line<'static>>,
    /// Rows the transcript gets once the plan, composer, and status line are placed.
    transcript_height: u16,
}

impl FrameLayout {
    fn new(area: Rect, app: &mut App, presenter: &mut Presenter) -> Self {
        let status_line_rows = status_line_height(app, area.width, app.status_line_settings(), presenter.theme());
        let composer_layout = app.composer().layout(area.width, presenter.theme());
        let completion_rows = rows(app.composer().completion_ref().map_or(0, |o| o.row_count(COMPLETION_MAX_ROWS)));
        let prompt_search_rows =
            rows(app.composer().prompt_search_ref().map_or(0, |p| p.height(PROMPT_SEARCH_MAX_ROWS)));
        let requested_composer_height =
            rows(composer_layout.lines.len()).saturating_add(completion_rows).saturating_add(prompt_search_rows);
        let composer_height = requested_composer_height.max(1).min(area.height.saturating_sub(status_line_rows));

        let remaining = area.height.saturating_sub(composer_height).saturating_sub(status_line_rows);
        let padding = app.content_padding();
        let plan_entries = app.plan_entries();
        // The plan never takes more than a third of what is left, so a long plan
        // cannot squeeze out the conversation.
        let plan_height =
            rows(PlanView::new(&plan_entries, presenter.theme(), padding).line_count()).min(remaining.div_ceil(3));
        let progress_lines = app.progress_indicator().lines(presenter.theme(), app.spinner_tick(), padding);
        let live_lines =
            if app.full_screen_active() { Vec::new() } else { live_history_lines(app, presenter, area.width) };

        Self {
            composer_layout,
            completion_rows,
            prompt_search_rows,
            composer_height,
            status_line_rows,
            plan_entries,
            plan_height,
            progress_lines,
            live_lines,
            transcript_height: remaining.saturating_sub(plan_height),
        }
    }

    /// Committed scrollback rows that still fit on screen; anything older is
    /// handed to the terminal's own history.
    fn committed_capacity(&self) -> usize {
        usize::from(self.transcript_height)
            .saturating_sub(self.live_lines.len())
            .saturating_sub(self.progress_lines.len())
    }
}

fn draw_frame(frame: &mut Frame, app: &mut App, presenter: &mut Presenter, layout: &FrameLayout) {
    let [live_area, composer_area, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(layout.composer_height),
        Constraint::Length(layout.status_line_rows),
    ])
    .areas(frame.area());

    let buf = frame.buffer_mut();
    let [_, plan_area, transcript_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(layout.plan_height),
        Constraint::Length(layout.transcript_height.min(live_area.height)),
    ])
    .areas(live_area);

    if layout.plan_height > 0 {
        PlanView::new(&layout.plan_entries, presenter.theme(), app.content_padding()).render(plan_area, buf);
    }
    draw_transcript(layout, presenter.committed_lines(), transcript_area, buf);

    let cursor = render_composer(layout, app, composer_area, buf, presenter.theme());
    StatusLine::new(app, app.status_line_settings(), presenter.theme()).render(status_area, buf);

    if let Some(cursor) = cursor {
        frame.set_cursor_position(cursor);
    }
    app.render_active_surface(frame, &mut presenter.context());
}

/// Stacks committed scrollback, the streaming tail, and the progress indicator
/// against the bottom of `area`, trimming the oldest rows first.
fn draw_transcript(layout: &FrameLayout, committed: &[Line<'static>], area: Rect, buf: &mut Buffer) {
    let mut remaining = usize::from(area.height);
    let progress = tail(&layout.progress_lines, &mut remaining);
    let live = tail(&layout.live_lines, &mut remaining);
    let committed = tail(committed, &mut remaining);

    let [committed_area, live_area, progress_area] = Layout::vertical([
        Constraint::Length(rows(committed.len())),
        Constraint::Length(rows(live.len())),
        Constraint::Length(rows(progress.len())),
    ])
    .areas(area);
    render_lines(committed, committed_area, buf);
    render_lines(live, live_area, buf);
    render_lines(progress, progress_area, buf);
}

/// The last `remaining` lines of `lines`, decrementing `remaining` by how many
/// were taken.
fn tail<'a>(lines: &'a [Line<'static>], remaining: &mut usize) -> &'a [Line<'static>] {
    let taken = lines.len().min(*remaining);
    *remaining -= taken;
    &lines[lines.len() - taken..]
}

/// Draws one line per row, avoiding the full copy a `Paragraph` would need.
fn render_lines(lines: &[Line<'static>], area: Rect, buf: &mut Buffer) {
    for (index, line) in lines.iter().take(usize::from(area.height)).enumerate() {
        line.render(Rect { y: area.y + rows(index), height: 1, ..area }, buf);
    }
}

fn rows(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

fn live_history_lines(app: &App, presenter: &mut Presenter, width: u16) -> Vec<Line<'static>> {
    presenter.pending_history_lines(
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
        Constraint::Min(0),
        Constraint::Length(layout.completion_rows),
    ])
    .areas(area);

    if search_area.height > 0 {
        if let Some(picker) = app.composer_mut().prompt_search() {
            picker.render(search_area, buf, theme);
        }
        app.set_surface_rect(search_area);
    }
    if completion_area.height > 0 {
        if let Some(overlay) = app.composer_mut().completion() {
            overlay.view(theme, completion_area.width).render(completion_area, buf);
        }
        app.set_surface_rect(completion_area);
    }

    // Keep the cursor line visible when the composer is taller than its area.
    let skip = layout.composer_layout.lines.len().saturating_sub(usize::from(body_area.height));
    render_lines(&layout.composer_layout.lines[skip..], body_area, buf);

    let cursor_row = usize::from(layout.composer_layout.cursor.y);
    if cursor_row < skip {
        return None;
    }
    let x = body_area.x.saturating_add(layout.composer_layout.cursor.x);
    let y = body_area.y.saturating_add(rows(cursor_row - skip));
    (x < body_area.right() && y < body_area.bottom()).then(|| Position::new(x, y))
}
