use crate::app::App;
use crate::plan_view::PlanView;
use crate::presentation::{Presenter, Segment};
use crate::status_line::StatusLine;
use crate::terminal::INLINE_SCROLLBACK_RESERVE;
use crate::theme::Theme;
use crate::widgets::render_rows;
use crate::wrap::as_u16;
use agent_client_protocol::schema::PlanEntry;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
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
        let lines = presenter.lines(
            Segment::Committed,
            &committed,
            previous_kind,
            area.width,
            app.content_padding(),
            app.spinner_tick(),
        );
        presenter.scrollback_mut().append(lines);
    }

    let layout = FrameLayout::new(area, app, presenter);

    // Scrollback leaves for the terminal's own history before the frame is
    // painted, so the viewport is drawn once, already in its final position.
    if commits_history {
        let overflow = presenter.scrollback_mut().take_overflow(layout.committed_capacity());
        for chunk in overflow.chunks(usize::from(u16::MAX)) {
            let chunk = chunk.to_vec();
            terminal.insert_before(as_u16(chunk.len()), move |buffer| {
                Paragraph::new(Text::from(chunk)).render(buffer.area, buffer);
            })?;
        }
    }

    terminal.draw(|frame| draw_frame(frame, app, presenter, &layout))?;
    Ok(())
}

/// Everything measured once per frame, before anything is drawn.
struct FrameLayout {
    composer_layout: crate::composer::ComposerLayout,
    completion_rows: u16,
    prompt_search_rows: u16,
    composer_height: u16,
    status_line: StatusLine,
    status_line_rows: u16,
    plan_entries: Vec<PlanEntry>,
    plan_height: u16,
    progress_lines: Vec<Line<'static>>,
    live_lines: Vec<Line<'static>>,
    /// Columns of blank gutter each side of the conversation content.
    content_padding: u16,
    /// Rows the conversation gets once the plan, composer, and status line are
    /// placed — what [`FrameLayout::split`]'s `Fill` resolves to.
    transcript_height: u16,
}

impl FrameLayout {
    fn new(area: Rect, app: &mut App, presenter: &mut Presenter) -> Self {
        let status_line = StatusLine::new(&app.status_line_model(), presenter.theme());
        let status_line_rows = status_line.height(area.width);
        let composer_layout = app.composer_mut().layout(area.width, presenter.theme());
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
            as_u16(PlanView::new(&plan_entries, presenter.theme()).line_count()).min(remaining.div_ceil(3));
        let progress_lines = app.progress_indicator().lines(presenter.theme(), app.spinner_tick());
        let live_lines =
            if app.full_screen_active() { Vec::new() } else { live_history_lines(app, presenter, area.width) };

        Self {
            composer_layout,
            completion_rows,
            prompt_search_rows,
            composer_height,
            status_line,
            status_line_rows,
            plan_entries,
            plan_height,
            progress_lines,
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
            .saturating_sub(self.progress_lines.len())
    }
}

fn draw_frame(frame: &mut Frame, app: &mut App, presenter: &mut Presenter, layout: &FrameLayout) {
    let [transcript_area, plan_area, composer_area, status_area] = layout.split(frame.area());
    let buf = frame.buffer_mut();

    PlanView::new(&layout.plan_entries, presenter.theme()).render(layout.indent(plan_area), buf);
    draw_transcript(layout, presenter.scrollback().lines(), transcript_area, buf);

    let cursor = render_composer(layout, app, composer_area, buf, presenter.theme());
    layout.status_line.render(status_area, buf);

    let layer_cursor = app.render_layer(frame.area(), frame.buffer_mut(), &mut presenter.context());
    if let Some(cursor) = layer_cursor.or(cursor) {
        frame.set_cursor_position(cursor);
    }
}

/// Stacks committed scrollback, the streaming tail, and the progress indicator
/// against the bottom of `area`, trimming the oldest rows first.
fn draw_transcript(layout: &FrameLayout, committed: &[Line<'static>], area: Rect, buf: &mut Buffer) {
    let mut remaining = usize::from(area.height);
    let progress = tail(&layout.progress_lines, &mut remaining);
    let live = tail(&layout.live_lines, &mut remaining);
    let committed = tail(committed, &mut remaining);

    let [committed_area, live_area, progress_area] = Layout::vertical([
        Constraint::Length(as_u16(committed.len())),
        Constraint::Length(as_u16(live.len())),
        Constraint::Length(as_u16(progress.len())),
    ])
    .areas(area);
    render_rows(committed, committed_area, buf);
    render_rows(live, live_area, buf);
    render_rows(progress, layout.indent(progress_area), buf);
}

/// The last `remaining` lines of `lines`, decrementing `remaining` by how many
/// were taken.
fn tail<'a>(lines: &'a [Line<'static>], remaining: &mut usize) -> &'a [Line<'static>] {
    let taken = lines.len().min(*remaining);
    *remaining -= taken;
    &lines[lines.len() - taken..]
}

fn live_history_lines(app: &App, presenter: &mut Presenter, width: u16) -> Vec<Line<'static>> {
    presenter.lines(
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
        overlay.view(theme).render(completion_area, buf);
    }

    // Keep the cursor line visible when the composer is taller than its area.
    let skip = layout.composer_layout.lines.len().saturating_sub(usize::from(body_area.height));
    render_rows(&layout.composer_layout.lines[skip..], body_area, buf);

    let cursor_row = usize::from(layout.composer_layout.cursor.y);
    if cursor_row < skip {
        return None;
    }
    let x = body_area.x.saturating_add(layout.composer_layout.cursor.x);
    let y = body_area.y.saturating_add(as_u16(cursor_row - skip));
    (x < body_area.right() && y < body_area.bottom()).then(|| Position::new(x, y))
}
