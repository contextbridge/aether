use crate::app::App;
use crate::conversation::plan_view::PlanView;
use crate::conversation::status_line::StatusLine;
use crate::surfaces::composer::ComposerLayout;
use crate::view::wrap::as_u16;
use agent_client_protocol::schema::v1::PlanEntry;
use ratatui::layout::{Constraint, Layout, Margin, Rect};

use super::Renderer;

/// Most rows the completion list and history search may claim.
const COMPLETION_MAX_ROWS: usize = 6;
const PROMPT_SEARCH_MAX_ROWS: usize = 12;

/// Everything measured once per frame, before anything is drawn.
pub(super) struct FrameLayout {
    pub(super) composer_layout: ComposerLayout,
    pub(super) completion_rows: u16,
    pub(super) prompt_search_rows: u16,
    pub(super) composer_height: u16,
    pub(super) status_line: StatusLine,
    pub(super) status_line_rows: u16,
    pub(super) plan_entries: Vec<PlanEntry>,
    pub(super) plan_height: u16,
    pub(super) progress_height: u16,
    /// Columns of blank gutter each side of the conversation content.
    pub(super) content_padding: u16,
    /// Rows the conversation gets once the plan, composer, and status line are
    /// placed — what [`FrameLayout::split`]'s `Fill` resolves to.
    pub(super) transcript_height: u16,
}

impl FrameLayout {
    pub(super) fn new(area: Rect, app: &App, renderer: &Renderer) -> Self {
        let status_line = StatusLine::new(&app.status_line_model(), renderer.theme());
        let status_line_rows = status_line.height(area.width);
        let composer_layout = app.composer().layout(area.width, renderer.theme());
        let completion_rows =
            as_u16(app.composer().completion().map_or(0, |overlay| overlay.row_count(COMPLETION_MAX_ROWS)));
        let prompt_search_rows =
            as_u16(app.composer().prompt_search().map_or(0, |picker| picker.height(PROMPT_SEARCH_MAX_ROWS)));
        let requested_composer_height =
            as_u16(composer_layout.lines.len()).saturating_add(completion_rows).saturating_add(prompt_search_rows);
        let composer_height = requested_composer_height.max(1).min(area.height.saturating_sub(status_line_rows));

        let remaining = area.height.saturating_sub(composer_height).saturating_sub(status_line_rows);
        let plan_entries = app.plan_entries();
        // The plan never takes more than a third of what is left, so a long plan
        // cannot squeeze out the conversation.
        let plan_height =
            as_u16(PlanView::new(&plan_entries, renderer.theme()).line_count()).min(remaining.div_ceil(3));
        let progress_height = app.progress_indicator().height();

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
            content_padding: as_u16(app.content_padding()),
            transcript_height: remaining.saturating_sub(plan_height),
        }
    }

    /// Splits the frame into the four stacked bands it always has. The plan and
    /// the conversation share whatever the composer and status line leave.
    pub(super) fn split(&self, area: Rect) -> [Rect; 4] {
        Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(self.plan_height),
            Constraint::Length(self.composer_height),
            Constraint::Length(self.status_line_rows),
        ])
        .areas(area)
    }

    /// `area` inset by the content gutter, for the widgets that draw inside it.
    pub(super) fn indent(&self, area: Rect) -> Rect {
        area.inner(Margin::new(self.content_padding, 0))
    }
}
