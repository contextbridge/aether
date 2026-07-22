use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

use crate::plan_review::{
    PlanDocument, ReviewComment, SourceMarkdownLine, compile_feedback, render_markdown_source_lines,
};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

#[allow(clippy::cast_possible_truncation)]
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
    text: String,
    cursor: usize,
}

pub struct PlanReviewScreen {
    title: String,
    document: PlanDocument,
    source_lines: Vec<SourceMarkdownLine>,
    cached_line_count: usize,
    comments: Vec<ReviewComment>,
    plan_scroll: usize,
    plan_cursor_line: usize,
    outline_cursor: usize,
    outline_scroll: usize,
    draft: Option<DraftComment>,
    focus: Focus,
    respond: Option<Box<dyn FnOnce(ElicitationResponse) + Send>>,
    last_area: Rect,
}

impl PlanReviewScreen {
    pub fn new(meta: PlanReviewElicitationMeta, responder: Responder<ElicitationResponse>) -> Self {
        let document = PlanDocument::parse(&meta.plan_path, &meta.markdown);
        Self {
            title: meta.title,
            document,
            source_lines: Vec::new(),
            cached_line_count: 0,
            comments: Vec::new(),
            plan_scroll: 0,
            plan_cursor_line: 0,
            outline_cursor: 0,
            outline_scroll: 0,
            draft: None,
            focus: Focus::Plan,
            respond: Some(Box::new(move |response| {
                let _ = responder.respond(response);
            })),
            last_area: Rect::new(0, 0, 120, 40),
        }
    }

    fn source_line_count(&self) -> usize {
        self.document.line_count()
    }

    fn source_line_max_index(&self) -> usize {
        self.source_line_count().saturating_sub(1)
    }

    fn ensure_source_lines_rendered(&mut self, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let current_count = self.source_line_count();
        if self.cached_line_count != current_count || self.source_lines.is_empty() {
            self.source_lines = render_markdown_source_lines(&self.document.markdown_text(), theme, highlighter);
            self.cached_line_count = current_count;
        }
        // Clamp plan_cursor_line in case document changed
        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
    }

    pub fn on_mouse_scroll_up(&mut self, _local_y: u16, _local_x: u16) {
        if self.focus == Focus::Plan {
            self.plan_scroll = self.plan_scroll.saturating_sub(3);
            self.plan_cursor_line = self.plan_cursor_line.saturating_sub(3);
        } else {
            self.outline_scroll = self.outline_scroll.saturating_sub(1);
            self.outline_cursor = self.outline_cursor.saturating_sub(1);
        }
    }

    pub fn on_mouse_scroll_down(&mut self, _local_y: u16, _local_x: u16) {
        if self.focus == Focus::Plan {
            self.plan_scroll = self.plan_scroll.saturating_add(3).min(self.source_line_max_index());
            self.plan_cursor_line = self.plan_cursor_line.saturating_add(3).min(self.source_line_max_index());
        } else {
            let sections = &self.document.outline;
            self.outline_scroll = (self.outline_scroll + 1).min(sections.len().saturating_sub(1));
            self.outline_cursor = (self.outline_cursor + 1).min(sections.len().saturating_sub(1));
        }
    }

    pub fn on_mouse_click(&mut self, local_y: u16, local_x: u16) {
        if local_y < 1 {
            return;
        }
        let body_y = local_y.saturating_sub(1);
        let area = self.last_area;
        let use_split = area.width >= MIN_SPLIT_WIDTH && !self.document.outline.is_empty();
        if use_split {
            let outline_width = u16::try_from(u32::from(area.width) * OUTLINE_FRACTION / OUTLINE_TOTAL).unwrap_or(0);
            if local_x < outline_width {
                self.focus = Focus::Outline;
                let sections = &self.document.outline;
                if !sections.is_empty() {
                    let target = (self.outline_scroll + body_y as usize).min(sections.len().saturating_sub(1));
                    self.outline_cursor = target;
                }
            } else {
                self.focus = Focus::Plan;
                self.plan_cursor_line = self.plan_scroll + body_y as usize;
            }
        } else {
            self.focus = Focus::Plan;
            self.plan_cursor_line = self.plan_scroll + body_y as usize;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if self.respond.is_none() {
            return true;
        }

        if self.draft.is_some() {
            return self.handle_draft_key(key);
        }

        match self.focus {
            Focus::Plan => self.handle_plan_key(key),
            Focus::Outline => self.handle_outline_key(key),
        }
    }

    fn handle_plan_key(&mut self, key: KeyEvent) -> bool {
        let max = self.source_line_max_index();
        match key.code {
            KeyCode::Esc => {
                self.respond(ElicitationAction::Cancel, None);
                true
            }
            KeyCode::Char('a') => {
                self.respond(ElicitationAction::Accept, Some(PlanReviewDecision::Approve.response_content(None)));
                true
            }
            KeyCode::Char('r') => {
                let feedback = compile_feedback(&self.document, &self.comments);
                self.respond(
                    ElicitationAction::Accept,
                    Some(PlanReviewDecision::Deny.response_content(Some(&feedback))),
                );
                true
            }
            KeyCode::Char('c') => {
                let line_no = self.plan_cursor_line + 1;
                self.draft = Some(DraftComment { line_no, text: String::new(), cursor: 0 });
                false
            }
            KeyCode::Char('u') => {
                self.comments.pop();
                false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.plan_cursor_line < max {
                    self.plan_cursor_line += 1;
                }
                false
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.plan_cursor_line = self.plan_cursor_line.saturating_sub(1);
                false
            }
            KeyCode::Char('g') => {
                self.plan_cursor_line = 0;
                false
            }
            KeyCode::Char('G') => {
                self.plan_cursor_line = max;
                false
            }
            KeyCode::Char('n') => {
                self.jump_next_heading();
                false
            }
            KeyCode::Char('p') => {
                self.jump_prev_heading();
                false
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if !self.document.outline.is_empty() {
                    self.focus = Focus::Outline;
                }
                false
            }
            _ => false,
        }
    }

    fn handle_outline_key(&mut self, key: KeyEvent) -> bool {
        let max = self.document.outline.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => {
                self.respond(ElicitationAction::Cancel, None);
                true
            }
            KeyCode::Char('a') => {
                self.respond(ElicitationAction::Accept, Some(PlanReviewDecision::Approve.response_content(None)));
                true
            }
            KeyCode::Char('r') => {
                let feedback = compile_feedback(&self.document, &self.comments);
                self.respond(
                    ElicitationAction::Accept,
                    Some(PlanReviewDecision::Deny.response_content(Some(&feedback))),
                );
                true
            }
            KeyCode::Char('u') => {
                self.comments.pop();
                false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.outline_cursor = self.outline_cursor.saturating_add(1).min(max);
                false
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.outline_cursor = self.outline_cursor.saturating_sub(1);
                false
            }
            KeyCode::Char('g') => {
                self.outline_cursor = 0;
                false
            }
            KeyCode::Char('G') => {
                self.outline_cursor = max;
                false
            }
            KeyCode::Enter => {
                if let Some(section) = self.document.outline.get(self.outline_cursor) {
                    self.plan_cursor_line = section.first_line_no.saturating_sub(1).min(self.source_line_max_index());
                    self.focus = Focus::Plan;
                }
                false
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = Focus::Plan;
                false
            }
            _ => false,
        }
    }

    fn handle_draft_key(&mut self, key: KeyEvent) -> bool {
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                self.draft = None;
                false
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut draft.text);
                let line_no = draft.line_no;
                self.draft = None;
                if !text.trim().is_empty() {
                    self.comments.push(ReviewComment::new(line_no, text));
                }
                false
            }
            KeyCode::Backspace => {
                if draft.cursor > 0 {
                    draft.text.remove(draft.cursor - 1);
                    draft.cursor -= 1;
                }
                false
            }
            KeyCode::Left => {
                draft.cursor = draft.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                draft.cursor = draft.cursor.min(draft.text.len());
                false
            }
            KeyCode::Home => {
                draft.cursor = 0;
                false
            }
            KeyCode::End => {
                draft.cursor = draft.text.len();
                false
            }
            KeyCode::Char(c) => {
                if !c.is_control() || c == ' ' {
                    draft.text.insert(draft.cursor, c);
                    draft.cursor += 1;
                }
                false
            }
            _ => false,
        }
    }

    fn jump_next_heading(&mut self) {
        let heading_lines: Vec<usize> =
            self.document.outline.iter().map(|s| s.first_line_no.saturating_sub(1)).collect();
        if let Some(&next) = heading_lines.iter().find(|&&ln| ln > self.plan_cursor_line) {
            self.plan_cursor_line = next.min(self.source_line_max_index());
        }
    }

    fn jump_prev_heading(&mut self) {
        let heading_lines: Vec<usize> =
            self.document.outline.iter().map(|s| s.first_line_no.saturating_sub(1)).collect();
        if let Some(&prev) = heading_lines.iter().rev().find(|&&ln| ln < self.plan_cursor_line) {
            self.plan_cursor_line = prev;
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        self.last_area = frame.area();
        self.ensure_source_lines_rendered(theme, highlighter);

        let area = frame.area();
        if area.height < 4 || area.width < 20 {
            let msg = Paragraph::new("Plan review view is too small").style(Style::new().fg(theme.error));
            frame.render_widget(msg, area);
            return;
        }

        let footer_height = 1;
        let body_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(footer_height));
        let footer_area = Rect::new(area.x, area.y + body_area.height, area.width, footer_height);

        let use_split = area.width >= MIN_SPLIT_WIDTH && !self.document.outline.is_empty();

        if use_split {
            let outline_width = (u32::from(area.width) * OUTLINE_FRACTION / OUTLINE_TOTAL) as u16;
            let plan_width = area.width.saturating_sub(outline_width);
            let outline_area = Rect::new(body_area.x, body_area.y, outline_width, body_area.height);
            let plan_area = Rect::new(body_area.x + outline_width, body_area.y, plan_width, body_area.height);

            self.render_outline(frame, outline_area, theme);
            self.render_plan(frame, plan_area, theme);
        } else {
            self.render_plan(frame, body_area, theme);
        }

        self.render_footer(frame, footer_area, theme, use_split);
    }

    fn render_outline(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let title_style = if self.focus == Focus::Outline {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(if self.focus == Focus::Outline { theme.accent } else { theme.muted }))
            .title(Line::styled(" Outline ", title_style));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let height = inner.height as usize;
        let inner_width = inner.width.saturating_sub(1) as usize;
        if height == 0 {
            return;
        }

        let sections = &self.document.outline;
        if sections.is_empty() {
            return;
        }

        // Clamp and adjust scroll
        self.outline_cursor = self.outline_cursor.min(sections.len().saturating_sub(1));
        if self.outline_cursor < self.outline_scroll {
            self.outline_scroll = self.outline_cursor;
        } else if self.outline_cursor >= self.outline_scroll + height {
            self.outline_scroll = self.outline_cursor.saturating_sub(height.saturating_sub(1));
        }

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
        for row in 0..height {
            let section_index = self.outline_scroll + row;
            if let Some(section) = sections.get(section_index) {
                let selected = section_index == self.outline_cursor;
                let marker = if selected { "> " } else { "  " };
                let style = if selected && self.focus == Focus::Outline {
                    Style::new().bg(theme.accent).fg(theme.background)
                } else if selected {
                    Style::new().fg(theme.accent)
                } else {
                    Style::default()
                };
                let indent = "  ".repeat(section.level.saturating_sub(1) as usize);
                let prefix = format!("{marker}{indent}");
                let available = inner_width.saturating_sub(prefix.len());
                let title = truncate_str(&section.title, available);
                let mut line = Line::default();
                line.push_span(Span::styled(format!("{prefix}{title}"), style));
                lines.push(line);
            } else {
                lines.push(Line::default());
            }
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn render_plan(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(if self.focus == Focus::Plan { theme.accent } else { theme.muted }))
            .title(format!(" {} ", self.title));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let height = inner.height as usize;
        if height == 0 {
            return;
        }

        if self.source_line_count() == 0 {
            frame.render_widget(Paragraph::new("Plan is empty").style(Style::new().fg(theme.muted)), inner);
            return;
        }

        let max_line_no = self.source_line_count().max(1);
        let gutter_width = digit_count(max_line_no) + 3;

        let content_width = inner.width.saturating_sub(gutter_width as u16);
        if content_width == 0 {
            return;
        }

        // Adjust scroll to keep cursor visible
        self.plan_cursor_line = self.plan_cursor_line.min(self.source_line_max_index());
        if self.plan_cursor_line < self.plan_scroll {
            self.plan_scroll = self.plan_cursor_line;
        }
        // Scroll forward when cursor goes below viewport (approximate: each source line = 1 visual row)
        // For more accuracy we'd need to track visual row count, but source-line scrolling is sufficient
        let visible_source_lines = height;
        if self.plan_cursor_line >= self.plan_scroll + visible_source_lines {
            self.plan_scroll = self.plan_cursor_line.saturating_sub(visible_source_lines.saturating_sub(1));
        }

        let plan_scroll = self.plan_scroll;
        let plan_cursor = self.plan_cursor_line;
        let comments = &self.comments;
        let draft = &self.draft;

        // Build visual rows from source-line rendered markdown
        let mut visual_rows: Vec<Line<'static>> = Vec::new();
        let mut cursor_visual_row: Option<usize> = None;

        for line_idx in plan_scroll..self.source_line_count() {
            let source_line_no = line_idx + 1;
            let is_cursor = line_idx == plan_cursor;

            if line_idx >= self.source_lines.len() {
                break;
            }
            let source = &self.source_lines[line_idx];
            let rendered_line = &source.line;

            let head_line = build_gutter(source_line_no, gutter_width, theme);
            let tail_line = build_tail_gutter(gutter_width);

            let wrapped = wrap_line_to_width(rendered_line, content_width as usize);

            for (wrap_idx, wrapped_line) in wrapped.into_iter().enumerate() {
                if wrap_idx == 0 {
                    let mut row = head_line.clone();
                    row.spans.extend(wrapped_line.spans);
                    if is_cursor {
                        cursor_visual_row = Some(visual_rows.len());
                    }
                    visual_rows.push(row);
                } else {
                    let mut row = tail_line.clone();
                    row.spans.extend(wrapped_line.spans);
                    visual_rows.push(row);
                }
            }

            // Insert submitted comment blocks after this source line
            let comments_for_line: Vec<&ReviewComment> =
                comments.iter().filter(|c| c.line_no == source_line_no).collect();
            for comment in &comments_for_line {
                // Header row
                let mut header = Line::default();
                header.push_span(Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)));
                header.push_span(Span::styled(
                    format!("┌ comment on line {source_line_no}"),
                    Style::new().fg(theme.info).add_modifier(Modifier::ITALIC),
                ));
                visual_rows.push(header);

                for body_line_text in comment.body.lines() {
                    let mut row = Line::default();
                    row.push_span(Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)));
                    row.push_span(Span::styled(format!("│ {body_line_text}"), Style::new().fg(theme.text_secondary)));
                    visual_rows.push(row);
                }
            }

            // Insert draft comment after this source line
            if let Some(draft) = draft
                && draft.line_no == source_line_no
            {
                let mut header = Line::default();
                header.push_span(Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)));
                header.push_span(Span::styled(
                    "┌ [new comment]",
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                ));
                visual_rows.push(header);

                let mut input_row = Line::default();
                input_row.push_span(Span::styled(" ".repeat(gutter_width), Style::new().fg(theme.muted)));
                if draft.text.is_empty() {
                    input_row.push_span(Span::styled("│ ", Style::new().fg(theme.muted)));
                    input_row
                        .push_span(Span::styled("│", Style::new().fg(theme.accent).add_modifier(Modifier::SLOW_BLINK)));
                } else {
                    let before = &draft.text[..draft.cursor.min(draft.text.len())];
                    let cursor_char =
                        if draft.cursor < draft.text.len() { &draft.text[draft.cursor..=draft.cursor] } else { " " };
                    let after = &draft.text[(draft.cursor + 1).min(draft.text.len())..];

                    input_row.push_span(Span::styled("│ ", Style::new().fg(theme.muted)));
                    input_row.push_span(Span::styled(before.to_string(), Style::new().fg(theme.text_primary)));
                    input_row.push_span(Span::styled(
                        cursor_char.to_string(),
                        Style::new().fg(theme.background).bg(theme.accent),
                    ));
                    input_row.push_span(Span::styled(after.to_string(), Style::new().fg(theme.text_primary)));
                }
                visual_rows.push(input_row);
            }
        }

        // Render visible portion
        let visible: Vec<Line<'static>> = visual_rows.into_iter().take(height).collect();
        let mut rendered: Vec<Line<'static>> = Vec::with_capacity(visible.len());
        for (i, line) in visible.into_iter().enumerate() {
            let is_cursor = cursor_visual_row == Some(i);
            if is_cursor && self.focus == Focus::Plan {
                let mut styled = Line::default();
                for span in line.spans {
                    styled.push_span(Span::styled(
                        span.content.to_string(),
                        span.style.patch(Style::new().bg(theme.accent).fg(theme.background)),
                    ));
                }
                rendered.push(styled);
            } else {
                rendered.push(line);
            }
        }

        frame.render_widget(Paragraph::new(rendered), inner);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme, has_outline: bool) {
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

        let paragraph = Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.sidebar_bg));
        let clear = Paragraph::new("").style(Style::new().bg(theme.sidebar_bg));
        frame.render_widget(clear, area);
        frame.render_widget(paragraph, area);
    }

    pub fn cancel(&mut self) {
        self.respond(ElicitationAction::Cancel, None);
    }

    #[doc(hidden)]
    pub fn from_parts(meta: PlanReviewElicitationMeta, respond: Box<dyn FnOnce(ElicitationResponse) + Send>) -> Self {
        let document = PlanDocument::parse(&meta.plan_path, &meta.markdown);
        Self {
            title: meta.title,
            document,
            source_lines: Vec::new(),
            cached_line_count: 0,
            comments: Vec::new(),
            plan_scroll: 0,
            plan_cursor_line: 0,
            outline_cursor: 0,
            outline_scroll: 0,
            draft: None,
            focus: Focus::Plan,
            respond: Some(respond),
            last_area: Rect::new(0, 0, 120, 40),
        }
    }

    fn respond(&mut self, action: ElicitationAction, content: Option<serde_json::Value>) {
        if let Some(respond) = self.respond.take() {
            respond(ElicitationResponse { action, content });
        }
    }
}

fn digit_count(value: usize) -> usize {
    if value == 0 {
        return 1;
    }
    let mut count = 0;
    let mut v = value;
    while v > 0 {
        v /= 10;
        count += 1;
    }
    count
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

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else if max_chars <= 1 {
        String::new()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

fn wrap_line_to_width(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }

    // Collect all text from spans
    let spans: Vec<(String, Style)> = line.spans.iter().map(|s| (s.content.to_string(), s.style)).collect();

    let full_text: String = spans.iter().map(|(text, _)| text.as_str()).collect();
    if full_text.chars().count() <= width {
        return vec![line.clone()];
    }

    // Build a flat character list with style associations
    let mut chars: Vec<(char, Style)> = Vec::new();
    for (text, style) in &spans {
        for ch in text.chars() {
            chars.push((ch, *style));
        }
    }

    let mut result: Vec<Line<'static>> = Vec::new();
    let mut pos = 0usize;
    let total = chars.len();

    while pos < total {
        let end = (pos + width).min(total);
        let chunk = &chars[pos..end];

        let mut line_spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0usize;
        while i < chunk.len() {
            let style = chunk[i].1;
            let mut text = String::new();
            while i < chunk.len() && chunk[i].1 == style {
                text.push(chunk[i].0);
                i += 1;
            }
            line_spans.push(Span::styled(text, style));
        }

        result.push(Line::from(line_spans));
        pos = end;
    }

    result
}
