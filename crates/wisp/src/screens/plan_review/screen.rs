use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

use crate::renderer::DrawContext;
use crate::screens::annotation::{
    AnnotatedRows, Draft, apply_draft_key, comment_body_width, comment_box, draft_body, paste_into_draft,
};
use crate::screens::plan_review::{
    PlanDocument, ReviewComment, SourceMarkdownLine, compile_feedback, render_markdown_source_lines,
};
use crate::screens::review::{DocumentPane, MOUSE_SCROLL_LINES, Pane, body_and_footer, focused_border, focused_title};
use crate::surfaces::elicitation::ElicitationResponder;
use crate::surfaces::input::{MouseAction, PlanReviewOutput, ReviewOutcome, UiEvent, is_composed_char, is_press};
use crate::view::list_view::ListView;
use crate::view::selection::{Direction, SelectionState, step_clamped};
use crate::theme::Theme;
use crate::view::widgets::key_hints;
use crate::view::wrap::{as_u16, wrap_line, wrap_text_char};

const MIN_SPLIT_WIDTH: u16 = 60;
/// Columns the `" │ "` between the line number and the text occupies.
const GUTTER_SEPARATOR_WIDTH: usize = 3;
const OUTLINE_FRACTION: u32 = 1;
const OUTLINE_TOTAL: u32 = 4;

pub struct PlanReviewScreen {
    title: String,
    document: PlanDocument,
    comments: Vec<ReviewComment>,
    plan: DocumentPane<usize>,
    outline_selection: SelectionState,
    draft: Option<Draft<usize>>,
    focus: Pane,
    responder: ElicitationResponder,
}

/// The plan as rendered rows, with review comments woven in.
type PlanRowSet = AnnotatedRows<usize>;

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: impl Into<ElicitationResponder>) -> Self {
        let responder = responder.into();
        let document = PlanDocument::parse(&meta.plan_path, &meta.markdown);
        let outline_selection = SelectionState::new(document.outline.len());
        Self {
            title: meta.title,
            document,
            comments: Vec::new(),
            plan: DocumentPane::default(),
            outline_selection,
            draft: None,
            focus: Pane::Document,
            responder,
        }
    }

    fn source_line_count(&self) -> usize {
        self.document.line_count()
    }

    fn source_line_max_index(&self) -> usize {
        self.source_line_count().saturating_sub(1)
    }

    /// Scrolls the plan by rendered rows, or moves the outline when it has
    /// focus instead.
    fn scroll(&mut self, direction: Direction) {
        if self.focus == Pane::Nav {
            self.move_outline(direction);
            return;
        }
        self.plan.scroll_by(direction, MOUSE_SCROLL_LINES);
    }

    fn move_cursor(&mut self, direction: Direction) {
        self.plan.cursor = step_clamped(self.plan.cursor, direction, 1, self.source_line_max_index());
    }

    /// Moves the outline selection, stopping at either end.
    fn move_outline(&mut self, direction: Direction) {
        self.outline_selection.step_clamped(self.document.outline.len(), direction, |_| true);
    }

    /// Focuses whichever pane was clicked and moves its cursor to the clicked
    /// row, hit-testing against the areas the last render recorded.
    fn click(&mut self, row: u16, column: u16) {
        let position = Position::new(column, row);
        if self.outline_selection.rows_area().contains(position) {
            self.focus = Pane::Nav;
            self.outline_selection.select_at(row, self.document.outline.len());
            return;
        }
        if self.plan.contains(position) {
            self.focus = Pane::Document;
            self.plan.focus_at(row);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PlanReviewOutput> {
        if self.responder.is_answered() {
            return vec![PlanReviewOutput::Outcome(ReviewOutcome::Cancelled)];
        }
        if self.draft.is_some() {
            self.handle_draft_key(key);
            return Vec::new();
        }
        // Draft editing chords remain available; all following bindings are plain keys.
        if is_composed_char(key) {
            return Vec::new();
        }
        if let Some(outcome) = self.decision_key(key) {
            return outcome;
        }
        match self.focus {
            Pane::Document => self.handle_plan_key(key),
            Pane::Nav => self.handle_outline_key(key),
        }
        Vec::new()
    }

    /// Keys that end the review, available from either pane. The decision
    /// itself travels through the elicitation responder; the returned outcome
    /// tells the root how the review ended.
    fn decision_key(&mut self, key: KeyEvent) -> Option<Vec<PlanReviewOutput>> {
        let outcome = match key.code {
            KeyCode::Esc => {
                self.responder.cancel();
                ReviewOutcome::Cancelled
            }
            KeyCode::Char('a') => {
                self.responder.accept_strings([("decision", PlanReviewDecision::Approve.as_str())]);
                ReviewOutcome::Submitted("Approved the plan".to_string())
            }
            KeyCode::Char('r') => {
                let feedback = compile_feedback(&self.document, &self.comments);
                self.responder
                    .accept_strings([("decision", PlanReviewDecision::Deny.as_str()), ("feedback", &feedback)]);
                ReviewOutcome::Submitted("Requested changes to the plan".to_string())
            }
            KeyCode::Char('u') => {
                self.comments.pop();
                return Some(Vec::new());
            }
            _ => return None,
        };
        Some(vec![PlanReviewOutput::Outcome(outcome)])
    }

    fn handle_plan_key(&mut self, key: KeyEvent) {
        let max = self.source_line_max_index();
        match key.code {
            KeyCode::Char('c') => self.draft = Some(Draft::new(self.plan.cursor + 1)),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(Direction::Forward),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(Direction::Backward),
            KeyCode::Char('g') => self.plan.cursor = 0,
            KeyCode::Char('G') => self.plan.cursor = max,
            KeyCode::Char('n') => self.jump_heading(Direction::Forward),
            KeyCode::Char('p') => self.jump_heading(Direction::Backward),
            KeyCode::Char('h') | KeyCode::Left if !self.document.outline.is_empty() => self.focus = Pane::Nav,
            _ => {}
        }
    }

    fn handle_outline_key(&mut self, key: KeyEvent) {
        let len = self.document.outline.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_outline(Direction::Forward),
            KeyCode::Char('k') | KeyCode::Up => self.move_outline(Direction::Backward),
            KeyCode::Char('g') => self.outline_selection.select_first(len),
            KeyCode::Char('G') => self.outline_selection.select(Some(len.saturating_sub(1)), len),
            KeyCode::Enter => {
                if let Some(section) =
                    self.outline_selection.selected().and_then(|selected| self.document.outline.get(selected))
                {
                    self.plan.cursor = section.first_line_no.saturating_sub(1).min(self.source_line_max_index());
                    self.focus = Pane::Document;
                }
            }
            KeyCode::Char('l') | KeyCode::Right => self.focus = Pane::Document,
            _ => {}
        }
    }

    fn handle_draft_key(&mut self, key: KeyEvent) {
        if let Some((line_no, body)) = apply_draft_key(&mut self.draft, key) {
            self.comments.push(ReviewComment::new(line_no, body));
        }
    }

    /// Moves the cursor to the next or previous section heading.
    fn jump_heading(&mut self, direction: Direction) {
        let headings = self.document.outline.iter().map(|section| section.first_line_no.saturating_sub(1));
        let target = match direction {
            Direction::Forward => headings.into_iter().find(|&line| line > self.plan.cursor),
            Direction::Backward => headings.into_iter().rfind(|&line| line < self.plan.cursor),
        };
        if let Some(line) = target {
            self.plan.cursor = line.min(self.source_line_max_index());
        }
    }

    fn render_screen(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        source_lines: &[SourceMarkdownLine],
    ) -> Option<Position> {
        if area.height < 4 || area.width < 20 {
            Paragraph::new("Plan review view is too small").style(Style::new().fg(theme.error)).render(area, buf);
            return None;
        }

        let (body_area, footer_area) = body_and_footer(area);
        let use_split = area.width >= MIN_SPLIT_WIDTH && !self.document.outline.is_empty();

        let cursor = if use_split {
            let [outline_area, plan_area] = Layout::horizontal([
                Constraint::Ratio(OUTLINE_FRACTION, OUTLINE_TOTAL),
                Constraint::Ratio(OUTLINE_TOTAL - OUTLINE_FRACTION, OUTLINE_TOTAL),
            ])
            .areas(body_area);

            self.render_outline(outline_area, buf, theme);
            self.render_plan(plan_area, buf, theme, source_lines)
        } else {
            self.render_plan(body_area, buf, theme, source_lines)
        };

        self.render_footer(footer_area, buf, theme, use_split);
        cursor
    }

    fn render_outline(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let focused = self.focus == Pane::Nav;
        let block = Block::bordered()
            .border_style(focused_border(focused, theme))
            .title(Line::styled(" Outline ", focused_title(focused, theme)));

        let rows = self
            .document
            .outline
            .iter()
            .map(|section| {
                let indent = "  ".repeat(usize::from(section.level.saturating_sub(1)));
                Line::raw(format!("  {indent}{}", section.title))
            })
            .collect();
        let highlight_style = if self.focus == Pane::Nav {
            Style::new().bg(theme.accent).fg(theme.background)
        } else {
            Style::new().fg(theme.accent)
        };
        let view =
            ListView::new(rows, theme).block(block).scrollbar().highlight_symbol("> ").highlight_style(highlight_style);
        StatefulWidget::render(view, area, buf, &mut self.outline_selection);
    }

    fn render_plan(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        source_lines: &[SourceMarkdownLine],
    ) -> Option<Position> {
        let focused = self.focus == Pane::Document;
        let block = Block::bordered().border_style(focused_border(focused, theme)).title(format!(" {} ", self.title));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 {
            return None;
        }
        if self.source_line_count() == 0 {
            Paragraph::new("Plan is empty").style(Style::new().fg(theme.muted)).render(inner, buf);
            return None;
        }

        let gutter_width = self
            .source_line_count()
            .checked_ilog10()
            .map_or(0, |digits| usize::try_from(digits).unwrap_or(0))
            + 1
            + GUTTER_SEPARATOR_WIDTH;
        let content_width = PlanRowSet::content_width(inner.width).saturating_sub(as_u16(gutter_width));
        if content_width == 0 {
            return None;
        }

        let mark_cursor = focused && self.draft.is_none();
        let rows = self.build_rows(gutter_width, content_width, theme, source_lines);
        self.plan.render_rows(rows, inner, buf, theme, mark_cursor);
        self.plan.draft_cursor_position()
    }

    /// Lays the plan out as rendered rows, with each comment and the draft woven
    /// in beneath the source line it is anchored to.
    fn build_rows(
        &self,
        gutter_width: usize,
        content_width: u16,
        theme: &Theme,
        source_lines: &[SourceMarkdownLine],
    ) -> PlanRowSet {
        let mut rows = PlanRowSet::default();

        for (index, source) in source_lines.iter().enumerate().take(self.source_line_count()) {
            let line_no = index + 1;

            for (wrap_index, wrapped) in wrap_line(source.line.clone(), content_width).into_iter().enumerate() {
                let mut row = if wrap_index == 0 {
                    let num_width = gutter_width.saturating_sub(GUTTER_SEPARATOR_WIDTH);
                    let mut line = Line::default();
                    line.push_span(Span::styled(format!("{line_no:>num_width$}"), Style::new().fg(theme.text_secondary)));
                    line.push_span(Span::styled(" │ ", Style::new().fg(theme.muted)));
                    line
                } else {
                    Line::from(Span::raw(" ".repeat(gutter_width)))
                };
                row.spans.extend(wrapped.spans);
                // Only the first row of a wrapped line takes the cursor, so
                // stepping by one always moves a whole source line.
                if wrap_index == 0 {
                    rows.push(row, index);
                } else {
                    rows.push_anchored(row, index);
                }
            }

            let box_width = content_width.saturating_add(as_u16(gutter_width));
            for comment in self.comments.iter().filter(|comment| comment.line_no == line_no) {
                rows.push_annotation(comment_box(
                    &format!("┌─ Comment on line {line_no} ─"),
                    &wrap_text_char(&comment.body, comment_body_width(box_width)),
                    theme.info,
                    box_width,
                    theme,
                ));
            }

            if let Some(draft) = self.draft.as_ref().filter(|draft| draft.anchor == line_no) {
                let (body, cursor) = draft_body(draft, comment_body_width(box_width));
                rows.push_draft(comment_box("┌ Draft ─", &body, theme.accent, box_width, theme), cursor);
            }
        }

        rows
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer, theme: &Theme, has_outline: bool) {
        let mut hints = vec![("j/k", "move")];
        if self.focus == Pane::Document {
            hints.push(("n/p", "heading"));
            if has_outline {
                hints.push(("h", "outline"));
            }
            hints.push(("c", "comment"));
        } else {
            hints.extend([("g/G", "top/end"), ("enter", "jump"), ("l", "plan")]);
        }
        hints.extend([("u", "undo"), ("a", "approve"), ("r", "changes"), ("Esc", "cancel")]);

        Paragraph::new(key_hints(&hints, theme)).style(Style::new().bg(theme.sidebar_bg)).render(area, buf);
    }
}

impl PlanReviewScreen {
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<PlanReviewOutput> {
        match event {
            UiEvent::Key(key) if is_press(key) => self.on_key(key),
            UiEvent::Key(_) => Vec::new(),
            UiEvent::Paste(text) => {
                paste_into_draft(&mut self.draft, &text);
                Vec::new()
            }
            UiEvent::Mouse(action, (column, row)) => self.on_mouse(action, row, column),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<PlanReviewOutput> {
        self.handle_key(key)
    }

    pub fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<PlanReviewOutput> {
        match action {
            MouseAction::ScrollUp => self.scroll(Direction::Backward),
            MouseAction::ScrollDown => self.scroll(Direction::Forward),
            MouseAction::Click => self.click(row, column),
        }
        Vec::new()
    }

    pub fn cancel(&mut self) {
        self.responder.cancel();
    }
}

impl PlanReviewScreen {
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        let source_lines = render_markdown_source_lines(&self.document.markdown_text(), cx.theme, cx.highlighter);
        self.render_screen(area, buf, cx.theme, &source_lines)
    }
}
