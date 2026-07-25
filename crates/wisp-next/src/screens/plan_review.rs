use acp_utils::notifications::ElicitationAction;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

use crate::annotation::{Draft, DraftOutcome};
use crate::edit_buffer::EditBuffer;
use crate::elicitation::ElicitationResponder;
use crate::generation::Generation;
use crate::list_view::ListView;
use crate::plan_review::{
    PlanDocument, ReviewComment, SourceMarkdownLine, compile_feedback, render_markdown_source_lines,
};
use crate::render_context::RenderContext;
use crate::selection::{Direction, SelectionState, scroll_into_view, step_clamped};
use crate::surface::{MouseAction, Surface, SurfaceMessage};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::widgets::{block_cursor_spans, render_vertical_scrollbar};
use crate::wrap::{rows as rows_u16, wrap_line};

const MIN_SPLIT_WIDTH: u16 = 60;
/// Lines a mouse wheel notch moves the focused pane.
const MOUSE_SCROLL_LINES: usize = 3;
/// Columns the `" │ "` between the line number and the text occupies.
const GUTTER_SEPARATOR_WIDTH: usize = 3;
const OUTLINE_FRACTION: u32 = 1;
const OUTLINE_TOTAL: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Outline,
    Plan,
}

pub struct PlanReviewScreen {
    title: String,
    document: PlanDocument,
    source_lines: Vec<SourceMarkdownLine>,
    source_presentation_theme_generation: Option<Generation>,
    comments: Vec<ReviewComment>,
    plan_scroll: usize,
    plan_cursor_line: usize,
    outline_selection: SelectionState,
    draft: Option<Draft<usize>>,
    focus: Focus,
    responder: ElicitationResponder,
    /// Where the plan's rows were last drawn, for hit-testing clicks.
    plan_rows_area: Rect,
}

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: impl Into<ElicitationResponder>) -> Self {
        let responder = responder.into();
        let document = PlanDocument::parse(&meta.plan_path, &meta.markdown);
        let outline_selection = SelectionState::new(document.outline.len());
        Self {
            title: meta.title,
            document,
            source_lines: Vec::new(),
            source_presentation_theme_generation: None,
            comments: Vec::new(),
            plan_scroll: 0,
            plan_cursor_line: 0,
            outline_selection,
            draft: None,
            focus: Focus::Plan,
            responder,
            plan_rows_area: Rect::ZERO,
        }
    }

    fn source_line_count(&self) -> usize {
        self.document.line_count()
    }

    fn source_line_max_index(&self) -> usize {
        self.source_line_count().saturating_sub(1)
    }

    fn ensure_source_presentation(
        &mut self,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        theme_generation: Generation,
    ) {
        if self.source_presentation_theme_generation == Some(theme_generation) {
            return;
        }

        self.source_lines = render_markdown_source_lines(&self.document.markdown_text(), theme, highlighter);
        self.source_presentation_theme_generation = Some(theme_generation);
        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
    }

    fn scroll(&mut self, direction: Direction) {
        if self.focus != Focus::Plan {
            self.move_outline(direction);
            return;
        }
        let max = self.source_line_max_index();
        self.plan_scroll = step_clamped(self.plan_scroll, direction, MOUSE_SCROLL_LINES, max);
        self.plan_cursor_line = step_clamped(self.plan_cursor_line, direction, MOUSE_SCROLL_LINES, max);
    }

    fn move_cursor(&mut self, direction: Direction) {
        self.plan_cursor_line = step_clamped(self.plan_cursor_line, direction, 1, self.source_line_max_index());
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
            self.focus = Focus::Outline;
            self.outline_selection.select_at(row, self.document.outline.len());
            return;
        }
        if self.plan_rows_area.contains(position) {
            self.focus = Focus::Plan;
            let offset = usize::from(row - self.plan_rows_area.y);
            self.plan_cursor_line = (self.plan_scroll + offset).min(self.source_line_max_index());
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<SurfaceMessage> {
        if self.responder.is_answered() {
            return vec![SurfaceMessage::Close];
        }
        if self.draft.is_some() {
            self.handle_draft_key(key);
            return Vec::new();
        }
        if let Some(outcome) = self.decision_key(key) {
            return outcome;
        }
        match self.focus {
            Focus::Plan => self.handle_plan_key(key),
            Focus::Outline => self.handle_outline_key(key),
        }
        Vec::new()
    }

    /// Keys that end the review, available from either pane.
    fn decision_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>> {
        match key.code {
            KeyCode::Esc => self.responder.cancel(),
            KeyCode::Char('a') => {
                self.responder
                    .respond(ElicitationAction::Accept, Some(PlanReviewDecision::Approve.response_content(None)));
            }
            KeyCode::Char('r') => {
                let feedback = compile_feedback(&self.document, &self.comments);
                self.responder.respond(
                    ElicitationAction::Accept,
                    Some(PlanReviewDecision::Deny.response_content(Some(&feedback))),
                );
            }
            KeyCode::Char('u') => {
                self.comments.pop();
                return Some(Vec::new());
            }
            _ => return None,
        }
        Some(vec![SurfaceMessage::Close])
    }

    fn handle_plan_key(&mut self, key: KeyEvent) {
        let max = self.source_line_max_index();
        match key.code {
            KeyCode::Char('c') => self.draft = Some(Draft::new(self.plan_cursor_line + 1)),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(Direction::Forward),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(Direction::Backward),
            KeyCode::Char('g') => self.plan_cursor_line = 0,
            KeyCode::Char('G') => self.plan_cursor_line = max,
            KeyCode::Char('n') => self.jump_heading(Direction::Forward),
            KeyCode::Char('p') => self.jump_heading(Direction::Backward),
            KeyCode::Char('h') | KeyCode::Left if !self.document.outline.is_empty() => self.focus = Focus::Outline,
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
                    self.plan_cursor_line = section.first_line_no.saturating_sub(1).min(self.source_line_max_index());
                    self.focus = Focus::Plan;
                }
            }
            KeyCode::Char('l') | KeyCode::Right => self.focus = Focus::Plan,
            _ => {}
        }
    }

    fn handle_draft_key(&mut self, key: KeyEvent) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let line_no = draft.anchor;
        match draft.on_key(key) {
            DraftOutcome::Continue => return,
            DraftOutcome::Commit(body) => self.comments.push(ReviewComment::new(line_no, body)),
            DraftOutcome::Discard => {}
        }
        self.draft = None;
    }

    /// Moves the cursor to the next or previous section heading.
    fn jump_heading(&mut self, direction: Direction) {
        let headings = self.document.outline.iter().map(|section| section.first_line_no.saturating_sub(1));
        let target = match direction {
            Direction::Forward => headings.into_iter().find(|&line| line > self.plan_cursor_line),
            Direction::Backward => headings.into_iter().rfind(|&line| line < self.plan_cursor_line),
        };
        if let Some(line) = target {
            self.plan_cursor_line = line.min(self.source_line_max_index());
        }
    }

    fn render_screen(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.height < 4 || area.width < 20 {
            Paragraph::new("Plan review view is too small").style(Style::new().fg(theme.error)).render(area, buf);
            return;
        }

        let [body_area, footer_area] = area.layout(&Layout::vertical([Constraint::Min(0), Constraint::Length(1)]));
        let use_split = self.uses_split(area);

        if use_split {
            let [outline_area, plan_area] = Layout::horizontal([
                Constraint::Ratio(OUTLINE_FRACTION, OUTLINE_TOTAL),
                Constraint::Ratio(OUTLINE_TOTAL - OUTLINE_FRACTION, OUTLINE_TOTAL),
            ])
            .areas(body_area);

            self.render_outline(outline_area, buf, theme);
            self.render_plan(plan_area, buf, theme);
        } else {
            // No outline at this width, so no rows for a click to land on.
            self.outline_selection.set_rows_area(Rect::ZERO);
            self.render_plan(body_area, buf, theme);
        }

        self.render_footer(footer_area, buf, theme, use_split);
    }

    /// The outline pane only earns its space on wide terminals with headings.
    fn uses_split(&self, area: Rect) -> bool {
        area.width >= MIN_SPLIT_WIDTH && !self.document.outline.is_empty()
    }

    fn render_outline(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let title_style = if self.focus == Focus::Outline {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)
        };

        let block = Block::bordered()
            .border_style(Style::new().fg(if self.focus == Focus::Outline { theme.accent } else { theme.muted }))
            .title(Line::styled(" Outline ", title_style));

        let rows = self
            .document
            .outline
            .iter()
            .map(|section| {
                let indent = "  ".repeat(usize::from(section.level.saturating_sub(1)));
                Line::raw(format!("  {indent}{}", section.title))
            })
            .collect();
        let highlight_style = if self.focus == Focus::Outline {
            Style::new().bg(theme.accent).fg(theme.background)
        } else {
            Style::new().fg(theme.accent)
        };
        ListView::new(rows, &mut self.outline_selection, theme)
            .block(block)
            .scrollbar()
            .highlight_symbol("> ")
            .highlight_style(highlight_style)
            .render(area, buf);
    }

    fn render_plan(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::bordered()
            .border_style(Style::new().fg(if self.focus == Focus::Plan { theme.accent } else { theme.muted }))
            .title(format!(" {} ", self.title));

        let inner = block.inner(area);
        block.render(area, buf);
        self.plan_rows_area = inner;

        let height = usize::from(inner.height);
        if height == 0 {
            return;
        }
        if self.source_line_count() == 0 {
            Paragraph::new("Plan is empty").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let gutter_width = digit_count(self.source_line_count().max(1)) + GUTTER_SEPARATOR_WIDTH;
        let content_width = inner.width.saturating_sub(u16::try_from(gutter_width).unwrap_or(u16::MAX));
        if content_width == 0 {
            return;
        }

        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
        self.plan_scroll = scroll_into_view(self.plan_scroll, self.plan_cursor_line, height);
        let (rows, source_rows, cursor_row) = self.build_rows(gutter_width, content_width, theme);

        // Comments and drafts push lines apart, so the scroll offset the rows
        // are drawn at is tracked in rendered rows rather than source lines.
        let source_scroll = source_rows.get(self.plan_scroll).copied().unwrap_or_default();
        let scroll = cursor_row.map_or(source_scroll, |row| scroll_into_view(source_scroll, row, height));

        let highlight = (self.focus == Focus::Plan).then_some(cursor_row).flatten();
        for (offset, line) in rows.iter().skip(scroll).take(height).enumerate() {
            let row = Rect { y: inner.y + rows_u16(offset), height: 1, ..inner };
            match highlight {
                Some(index) if index == scroll + offset => highlight_row(line.clone(), theme).render(row, buf),
                _ => line.render(row, buf),
            }
        }
        render_vertical_scrollbar(inner, buf, rows.len(), scroll);
    }

    /// Lays the plan out as rendered rows, returning them alongside the row each
    /// source line starts on and the row holding the cursor.
    fn build_rows(
        &self,
        gutter_width: usize,
        content_width: u16,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, Vec<usize>, Option<usize>) {
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut source_rows = Vec::with_capacity(self.source_line_count());
        let mut cursor_row = None;

        for (index, source) in self.source_lines.iter().enumerate().take(self.source_line_count()) {
            source_rows.push(rows.len());
            let line_no = index + 1;

            for (wrap_index, wrapped) in wrap_line(source.line.clone(), content_width).into_iter().enumerate() {
                let mut row = if wrap_index == 0 {
                    if index == self.plan_cursor_line {
                        cursor_row = Some(rows.len());
                    }
                    build_gutter(line_no, gutter_width, theme)
                } else {
                    build_tail_gutter(gutter_width)
                };
                row.spans.extend(wrapped.spans);
                rows.push(row);
            }

            for comment in self.comments.iter().filter(|comment| comment.line_no == line_no) {
                rows.push(annotation_row(
                    gutter_width,
                    &format!("┌ comment on line {line_no}"),
                    Style::new().fg(theme.info).add_modifier(Modifier::ITALIC),
                    theme,
                ));
                rows.extend(comment.body.lines().map(|text| {
                    annotation_row(gutter_width, &format!("│ {text}"), Style::new().fg(theme.text_secondary), theme)
                }));
            }

            if let Some(draft) = self.draft.as_ref().filter(|draft| draft.anchor == line_no) {
                rows.push(annotation_row(
                    gutter_width,
                    "┌ [new comment]",
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                    theme,
                ));
                rows.push(draft_row(&draft.buffer, gutter_width, theme));
            }
        }

        (rows, source_rows, cursor_row)
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer, theme: &Theme, has_outline: bool) {
        let plan_focused = self.focus == Focus::Plan;
        let mut spans: Vec<Span<'static>> = Vec::new();

        if plan_focused {
            add_hint(&mut spans, "j/k", "move", theme);
            add_hint(&mut spans, "n/p", "heading", theme);
            if has_outline {
                add_hint(&mut spans, "h", "outline", theme);
            }
            add_hint(&mut spans, "c", "comment", theme);
            add_hint(&mut spans, "u", "undo", theme);
        } else {
            add_hint(&mut spans, "j/k", "move", theme);
            add_hint(&mut spans, "g/G", "top/end", theme);
            add_hint(&mut spans, "enter", "jump", theme);
            add_hint(&mut spans, "l", "plan", theme);
            add_hint(&mut spans, "u", "undo", theme);
        }

        add_hint(&mut spans, "a", "approve", theme);
        add_hint(&mut spans, "r", "changes", theme);
        add_hint(&mut spans, "Esc", "cancel", theme);

        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.sidebar_bg)).render(area, buf);
    }
}

impl Surface for PlanReviewScreen {
    /// The screen owns every key, so nothing falls through to the shared list
    /// navigation.
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>> {
        Some(self.handle_key(key))
    }

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<SurfaceMessage> {
        match action {
            MouseAction::ScrollUp => self.scroll(Direction::Backward),
            MouseAction::ScrollDown => self.scroll(Direction::Forward),
            MouseAction::Click => self.click(row, column),
        }
        Vec::new()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        self.ensure_source_presentation(cx.theme, cx.highlighter, cx.theme_generation);
        self.render_screen(area, buf, cx.theme);
        None
    }

    fn cancel(&mut self) {
        self.responder.cancel();
    }
}

/// A line drawn beside the gutter rather than in it: comments and drafts.
fn annotation_row(gutter_width: usize, text: &str, style: Style, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)),
        Span::styled(text.to_string(), style),
    ])
}

/// The draft comment being typed, with a block cursor at the insertion point.
fn draft_row(buffer: &EditBuffer, gutter_width: usize, theme: &Theme) -> Line<'static> {
    let mut row = Line::from(vec![
        Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)),
        Span::styled("│ ", Style::new().fg(theme.muted)),
    ]);
    if buffer.is_empty() {
        row.push_span(Span::styled("│", Style::new().fg(theme.accent).add_modifier(Modifier::SLOW_BLINK)));
        return row;
    }

    row.spans.extend(block_cursor_spans(
        buffer,
        Style::new().fg(theme.text_primary),
        Style::new().fg(theme.background).bg(theme.accent),
    ));
    row
}

/// Repaints a whole row in the cursor colours.
fn highlight_row(line: Line<'static>, theme: &Theme) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|span| {
                Span::styled(span.content, span.style.patch(Style::new().bg(theme.accent).fg(theme.background)))
            })
            .collect::<Vec<_>>(),
    )
}

fn digit_count(value: usize) -> usize {
    value.checked_ilog10().map_or(0, |digits| usize::try_from(digits).unwrap_or(0)) + 1
}

fn build_gutter(line_no: usize, width: usize, theme: &Theme) -> Line<'static> {
    let num_width = width.saturating_sub(GUTTER_SEPARATOR_WIDTH);
    let mut line = Line::default();
    line.push_span(Span::styled(format!("{line_no:>num_width$}"), Style::new().fg(theme.text_secondary)));
    line.push_span(Span::styled(" │ ", Style::new().fg(theme.muted)));
    line
}

fn build_tail_gutter(width: usize) -> Line<'static> {
    Line::from(Span::raw(" ".repeat(width)))
}

fn add_hint(spans: &mut Vec<Span<'static>>, key: &str, desc: &str, theme: &Theme) {
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(key.to_string(), Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)));
    spans.push(Span::styled(format!(" {desc}"), Style::new().fg(theme.muted)));
}
