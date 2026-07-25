use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

use crate::edit_buffer::{EditBuffer, apply_edit_key};
use crate::plan_review::{
    PlanDocument, ReviewComment, SourceMarkdownLine, compile_feedback, render_markdown_source_lines,
};
use crate::screens::{MouseAction, RenderContext, Screen, ScreenOutcome};
use crate::selection::{Direction, SelectionState};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::{truncate_to_width, wrap_line};

const MIN_SPLIT_WIDTH: u16 = 60;
const OUTLINE_FRACTION: u32 = 1;
const OUTLINE_TOTAL: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Outline,
    Plan,
}

struct DraftComment {
    line_no: usize,
    buffer: EditBuffer,
}

pub struct PlanReviewScreen {
    title: String,
    document: PlanDocument,
    source_lines: Vec<SourceMarkdownLine>,
    source_presentation_theme_generation: Option<u64>,
    comments: Vec<ReviewComment>,
    plan_scroll: usize,
    plan_cursor_line: usize,
    outline_selection: SelectionState,
    draft: Option<DraftComment>,
    focus: Focus,
    respond: Option<Box<dyn FnOnce(ElicitationResponse) + Send>>,
    last_area: Rect,
}

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: Responder<ElicitationResponse>) -> Self {
        Self::new_with_response_handler(meta, move |response| {
            let _ = responder.respond(response);
        })
    }

    pub fn new_with_response_handler(
        meta: PlanReviewElicitationMeta,
        respond: impl FnOnce(ElicitationResponse) + Send + 'static,
    ) -> Self {
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
            respond: Some(Box::new(respond)),
            last_area: Rect::new(0, 0, 120, 40),
        }
    }

    fn source_line_count(&self) -> usize {
        self.document.line_count()
    }

    fn source_line_max_index(&self) -> usize {
        self.source_line_count().saturating_sub(1)
    }

    pub fn invalidate_source_presentation(&mut self) {
        self.source_lines.clear();
        self.source_presentation_theme_generation = None;
    }

    pub fn source_presentation_is_cached(&self) -> bool {
        self.source_presentation_theme_generation.is_some()
    }

    fn ensure_source_presentation(
        &mut self,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        theme_generation: u64,
    ) {
        if self.source_presentation_theme_generation == Some(theme_generation) {
            return;
        }

        self.source_lines = render_markdown_source_lines(&self.document.markdown_text(), theme, highlighter);
        self.source_presentation_theme_generation = Some(theme_generation);
        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
    }

    fn scroll(&mut self, direction: i32) {
        let step = 3 * direction;
        if self.focus == Focus::Plan {
            self.plan_scroll = offset_by(self.plan_scroll, step, self.source_line_max_index());
            self.plan_cursor_line = offset_by(self.plan_cursor_line, step, self.source_line_max_index());
        } else {
            let len = self.document.outline.len();
            let selected =
                offset_by(self.outline_selection.selected().unwrap_or_default(), direction, len.saturating_sub(1));
            self.outline_selection.select(Some(selected), len);
        }
    }

    fn click(&mut self, row: u16, column: u16) {
        let Some(body_row) = row.checked_sub(1).map(usize::from) else {
            return;
        };
        let area = self.last_area;
        let outline_width = u16::try_from(u32::from(area.width) * OUTLINE_FRACTION / OUTLINE_TOTAL).unwrap_or(0);
        if self.uses_split(area) && column < outline_width {
            self.focus = Focus::Outline;
            let sections = self.document.outline.len();
            if sections > 0 {
                self.outline_selection.select_row(body_row, sections);
            }
        } else {
            self.focus = Focus::Plan;
            self.plan_cursor_line = (self.plan_scroll + body_row).min(self.source_line_max_index());
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        if self.respond.is_none() {
            return ScreenOutcome::Close;
        }
        if self.draft.is_some() {
            self.handle_draft_key(key);
            return ScreenOutcome::None;
        }
        if let Some(outcome) = self.decision_key(key) {
            return outcome;
        }
        match self.focus {
            Focus::Plan => self.handle_plan_key(key),
            Focus::Outline => self.handle_outline_key(key),
        }
        ScreenOutcome::None
    }

    /// Keys that end the review, available from either pane.
    fn decision_key(&mut self, key: KeyEvent) -> Option<ScreenOutcome> {
        match key.code {
            KeyCode::Esc => self.respond(ElicitationAction::Cancel, None),
            KeyCode::Char('a') => {
                self.respond(ElicitationAction::Accept, Some(PlanReviewDecision::Approve.response_content(None)));
            }
            KeyCode::Char('r') => {
                let feedback = compile_feedback(&self.document, &self.comments);
                self.respond(
                    ElicitationAction::Accept,
                    Some(PlanReviewDecision::Deny.response_content(Some(&feedback))),
                );
            }
            KeyCode::Char('u') => {
                self.comments.pop();
                return Some(ScreenOutcome::None);
            }
            _ => return None,
        }
        Some(ScreenOutcome::Close)
    }

    fn handle_plan_key(&mut self, key: KeyEvent) {
        let max = self.source_line_max_index();
        match key.code {
            KeyCode::Char('c') => {
                self.draft = Some(DraftComment { line_no: self.plan_cursor_line + 1, buffer: EditBuffer::default() });
            }
            KeyCode::Char('j') | KeyCode::Down => self.plan_cursor_line = (self.plan_cursor_line + 1).min(max),
            KeyCode::Char('k') | KeyCode::Up => self.plan_cursor_line = self.plan_cursor_line.saturating_sub(1),
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
        let max = len.saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let selected = self.outline_selection.selected().unwrap_or_default().saturating_add(1).min(max);
                self.outline_selection.select(Some(selected), len);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let selected = self.outline_selection.selected().unwrap_or_default().saturating_sub(1);
                self.outline_selection.select(Some(selected), len);
            }
            KeyCode::Char('g') => self.outline_selection.select_first(len),
            KeyCode::Char('G') => self.outline_selection.select(Some(max), len),
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
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Enter => {
                let text = draft.buffer.take();
                let line_no = draft.line_no;
                self.draft = None;
                if !text.trim().is_empty() {
                    self.comments.push(ReviewComment::new(line_no, text));
                }
            }
            _ => {
                apply_edit_key(&mut draft.buffer, key);
            }
        }
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

    #[allow(clippy::cast_possible_truncation)]
    fn render_screen(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.last_area = area;
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

        let inner = block.inner(area);
        block.render(area, buf);

        let height = inner.height as usize;
        if height == 0 || self.document.outline.is_empty() {
            return;
        }
        self.outline_selection.ensure_visible(self.document.outline.len(), height);

        let inner_width = inner.width.saturating_sub(1) as usize;
        let items = self.document.outline.iter().map(|section| {
            let indent = "  ".repeat(section.level.saturating_sub(1) as usize);
            let prefix = format!("  {indent}");
            let available = inner_width.saturating_sub(prefix.len());
            ListItem::new(format!("{prefix}{}", truncate_to_width(&section.title, available)))
        });
        let highlight_style = if self.focus == Focus::Outline {
            Style::new().bg(theme.accent).fg(theme.background)
        } else {
            Style::new().fg(theme.accent)
        };
        let list = List::new(items).highlight_symbol("> ").highlight_style(highlight_style);
        StatefulWidget::render(list, inner, buf, self.outline_selection.list_state_mut());

        let mut scrollbar_state =
            ScrollbarState::new(self.document.outline.len()).position(self.outline_selection.offset());
        StatefulWidget::render(Scrollbar::new(ScrollbarOrientation::VerticalRight), inner, buf, &mut scrollbar_state);
    }

    #[allow(clippy::cast_possible_truncation)]
    fn render_plan(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::bordered()
            .border_style(Style::new().fg(if self.focus == Focus::Plan { theme.accent } else { theme.muted }))
            .title(format!(" {} ", self.title));

        let inner = block.inner(area);
        block.render(area, buf);

        let height = inner.height as usize;
        if height == 0 {
            return;
        }
        if self.source_line_count() == 0 {
            Paragraph::new("Plan is empty").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let gutter_width = digit_count(self.source_line_count().max(1)) + 3;
        let content_width = inner.width.saturating_sub(gutter_width as u16);
        if content_width == 0 {
            return;
        }

        self.keep_cursor_in_view(height);
        let (rows, source_rows, cursor_row) = self.build_rows(gutter_width, content_width, theme);

        // Comments and drafts push lines apart, so the scroll offset is tracked
        // in rendered rows rather than source lines.
        let mut scroll = source_rows.get(self.plan_scroll).copied().unwrap_or_default();
        if let Some(cursor_row) = cursor_row {
            if cursor_row < scroll {
                scroll = cursor_row;
            } else if cursor_row >= scroll + height {
                scroll = cursor_row.saturating_sub(height.saturating_sub(1));
            }
        }

        let row_count = rows.len();
        let highlight = (self.focus == Focus::Plan).then_some(cursor_row).flatten();
        let rows: Vec<Line<'static>> = rows
            .into_iter()
            .enumerate()
            .map(|(index, line)| if Some(index) == highlight { highlight_row(line, theme) } else { line })
            .collect();

        Paragraph::new(rows).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)).render(inner, buf);
        let mut scrollbar_state = ScrollbarState::new(row_count).position(scroll);
        StatefulWidget::render(Scrollbar::new(ScrollbarOrientation::VerticalRight), inner, buf, &mut scrollbar_state);
    }

    /// Scrolls just enough to bring the cursor line back into view.
    fn keep_cursor_in_view(&mut self, height: usize) {
        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
        if self.plan_cursor_line < self.plan_scroll {
            self.plan_scroll = self.plan_cursor_line;
        }
        if self.plan_cursor_line >= self.plan_scroll + height {
            self.plan_scroll = self.plan_cursor_line.saturating_sub(height.saturating_sub(1));
        }
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

            if let Some(draft) = self.draft.as_ref().filter(|draft| draft.line_no == line_no) {
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

    fn respond(&mut self, action: ElicitationAction, content: Option<serde_json::Value>) {
        if let Some(respond) = self.respond.take() {
            respond(ElicitationResponse { action, content });
        }
    }
}

impl Screen for PlanReviewScreen {
    fn on_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        self.handle_key(key)
    }

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) {
        match action {
            MouseAction::ScrollUp => self.scroll(-1),
            MouseAction::ScrollDown => self.scroll(1),
            MouseAction::Click => self.click(row, column),
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        self.ensure_source_presentation(cx.theme, cx.highlighter, cx.theme_generation);
        self.render_screen(area, buf, cx.theme);
        None
    }

    fn cancel(&mut self) {
        self.respond(ElicitationAction::Cancel, None);
    }
}

/// Shifts `value` by `delta` rows, saturating at zero and `max`.
fn offset_by(value: usize, delta: i32, max: usize) -> usize {
    value.saturating_add_signed(delta as isize).min(max)
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

    let text = buffer.text();
    let cursor = buffer.cursor();
    let cursor_len = text[cursor..].chars().next().map_or(0, char::len_utf8);
    let under_cursor = if cursor_len == 0 { " " } else { &text[cursor..cursor + cursor_len] };
    row.push_span(Span::styled(text[..cursor].to_string(), Style::new().fg(theme.text_primary)));
    row.push_span(Span::styled(under_cursor.to_string(), Style::new().fg(theme.background).bg(theme.accent)));
    row.push_span(Span::styled(text[cursor + cursor_len..].to_string(), Style::new().fg(theme.text_primary)));
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
    value.checked_ilog10().map_or(1, |digits| digits as usize + 1)
}

fn build_gutter(line_no: usize, width: usize, theme: &Theme) -> Line<'static> {
    let num_width = width.saturating_sub(3);
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
