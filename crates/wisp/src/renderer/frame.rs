use super::Renderer;
use super::layout::FrameLayout;
use crate::app::App;
use crate::conversation::plan_view::PlanView;
use crate::conversation::progress_indicator::{ProgressIndicator, ProgressIndicatorView};
use crate::surfaces::composer::ComposerBodyView;
use crate::theme::Theme;
use crate::view::widgets::RowsView;
use crate::view::wrap::as_u16;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{StatefulWidget, Widget};

pub(super) fn draw_frame(
    frame: &mut Frame,
    app: &mut App,
    renderer: &mut Renderer,
    layout: &FrameLayout,
    live: &[Line<'static>],
) {
    let [transcript_area, plan_area, composer_area, status_area] = layout.split(frame.area());
    let theme = renderer.theme();
    let buf = frame.buffer_mut();

    PlanView::new(&layout.plan_entries, theme).render(layout.indent(plan_area), buf);
    draw_transcript(layout, live, transcript_area, buf, app.progress_indicator(), theme, app.spinner_tick());

    let cursor = render_composer(layout, app, composer_area, buf, theme);
    layout.status_line.render(status_area, buf);

    let route_cursor = app.render_route(frame.area(), frame.buffer_mut(), &mut renderer.context());
    let overlay_cursor = app.render_overlay(frame.area(), frame.buffer_mut(), &mut renderer.context());
    let cursor = if app.has_modal() {
        overlay_cursor
    } else if app.full_screen_active() {
        route_cursor
    } else {
        cursor
    };

    if let Some(cursor) = cursor {
        frame.set_cursor_position(cursor);
    }
}

/// Stacks the streaming tail and progress band against the bottom of `area`,
/// trimming the oldest live rows first. Sealed rows are inserted into the
/// terminal's native scrollback before the viewport is drawn.
fn draw_transcript(
    layout: &FrameLayout,
    live_lines: &[Line<'static>],
    area: Rect,
    buf: &mut Buffer,
    progress: &ProgressIndicator,
    theme: &Theme,
    spinner_tick: usize,
) {
    let mut remaining = usize::from(area.height);
    let progress_height = usize::from(layout.progress_height).min(remaining);
    remaining = remaining.saturating_sub(progress_height);
    let taken = live_lines.len().min(remaining);
    let live = &live_lines[live_lines.len() - taken..];
    let [live_area, progress_area] =
        Layout::vertical([Constraint::Length(as_u16(live.len())), Constraint::Length(as_u16(progress_height))])
            .areas(area);
    RowsView::new(live).render(live_area, buf);
    ProgressIndicatorView::new(progress, theme, spinner_tick).render(layout.indent(progress_area), buf);
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
        && let Some(picker) = app.composer_mut().prompt_search_mut()
    {
        picker.render(search_area, buf, theme);
    }
    if completion_area.height > 0
        && let Some(overlay) = app.composer_mut().completion_mut()
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
