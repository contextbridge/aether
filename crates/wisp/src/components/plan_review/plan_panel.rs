use super::PlanDocument;
use crate::components::common::{AnchoredSurfaceBuilder, CachedLayer};
use crate::components::review_comments::{
    AnchorIndex, AnchoredRows, CommentAnchor, KeyOutcome, Navigation, ReviewComment, ReviewSurface, ReviewSurfaceEvent,
};
use tui::{
    Component, Event, Frame, Line, MouseEventKind, SourceMarkdownLine, Style, ViewContext, digit_count,
    render_markdown_source_lines,
};

const PAGE_SIZE: usize = 10;

pub enum PlanPanelMessage {}

pub(crate) type SourceLineAnchor = CommentAnchor<usize>;

pub struct PlanPanel {
    document: PlanDocument,
    surface: ReviewSurface<usize>,
    queued_comments: Vec<ReviewComment<usize>>,
    heading_lines: Vec<usize>,
    cached_markdown: CachedLayer<usize, PlanMarkdown>,
    cached_surface: CachedLayer<u16, PlanSurface>,
}

struct PlanMarkdown {
    rendered_lines: Vec<SourceMarkdownLine>,
}

struct PlanSurface {
    surface: AnchoredRows<usize>,
    line_anchors: AnchorIndex<usize>,
}

impl PlanPanel {
    pub fn new(document: PlanDocument) -> Self {
        let heading_lines = document.outline.iter().map(|section| section.first_line_no).collect();
        Self {
            document,
            surface: ReviewSurface::new(),
            queued_comments: Vec::new(),
            heading_lines,
            cached_markdown: CachedLayer::new(),
            cached_surface: CachedLayer::new(),
        }
    }

    pub fn document(&self) -> &PlanDocument {
        &self.document
    }

    pub fn is_in_comment_mode(&self) -> bool {
        self.surface.is_in_comment_mode()
    }

    pub fn current_source_line_no(&self) -> usize {
        self.current_cursor_source_line_no().unwrap_or(1)
    }

    pub fn set_cursor_source_line_no(&mut self, line_no: usize) {
        let Some(cached) = self.cached_surface.get() else {
            return;
        };

        if let Some(row) = cached.surface.start_row_for_anchor(CommentAnchor(line_no)) {
            self.surface.cursor_mut().row = row;
            return;
        }

        if let Some(nearest) =
            cached.line_anchors.as_slice().iter().rev().copied().find(|CommentAnchor(a)| *a <= line_no)
            && let Some(row) = cached.surface.start_row_for_anchor(nearest)
        {
            self.surface.cursor_mut().row = row;
        }
    }

    pub fn jump_next_heading(&mut self) -> bool {
        let current = self.current_source_line_no();
        if let Some(next) = self.heading_lines.iter().copied().find(|line_no| *line_no > current) {
            self.set_cursor_source_line_no(next);
            return true;
        }
        false
    }

    pub fn jump_prev_heading(&mut self) -> bool {
        let current = self.current_source_line_no();
        if let Some(previous) = self.heading_lines.iter().copied().rev().find(|line_no| *line_no < current) {
            self.set_cursor_source_line_no(previous);
            return true;
        }
        false
    }

    pub fn undo_last_comment(&mut self) -> bool {
        self.queued_comments.pop().is_some()
    }

    pub fn comment_count(&self) -> usize {
        self.queued_comments.len()
    }

    pub(crate) fn comments(&self) -> &[ReviewComment<usize>] {
        &self.queued_comments
    }

    fn current_cursor_anchor(&self) -> Option<SourceLineAnchor> {
        self.cached_surface.get().and_then(|cached| self.surface.current_anchor(&cached.surface))
    }

    fn current_cursor_source_line_no(&self) -> Option<usize> {
        self.current_cursor_anchor().map(|CommentAnchor(line_no)| line_no)
    }

    fn ensure_rendered(&mut self, ctx: &ViewContext) {
        self.ensure_cached_markdown(ctx);
        self.ensure_cached_surface(ctx);
    }

    fn ensure_cached_markdown(&mut self, ctx: &ViewContext) {
        self.cached_markdown.ensure(self.document.line_count(), || {
            let result = render_markdown_source_lines(&self.document.markdown_text(), ctx);
            PlanMarkdown { rendered_lines: result.lines }
        });
    }

    fn ensure_cached_surface(&mut self, ctx: &ViewContext) {
        let width = ctx.size.width;
        let cached = self.cached_markdown.get().expect("markdown cache populated above");
        let document = &self.document;
        self.cached_surface.ensure(width, || {
            let (surface, line_anchors) = build_plan_surface(document, &cached.rendered_lines, ctx);
            PlanSurface { surface, line_anchors }
        });
    }
}

impl Component for PlanPanel {
    type Message = PlanPanelMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let cached = self.cached_surface.get()?;
        let rows = &cached.surface;
        let nav = Navigation::AnchorStep { anchors: &cached.line_anchors, page_size: PAGE_SIZE };

        if let Event::Mouse(mouse) = event {
            return match mouse.kind {
                MouseEventKind::ScrollUp if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(-1, rows, nav);
                    Some(vec![])
                }
                MouseEventKind::ScrollDown if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(1, rows, nav);
                    Some(vec![])
                }
                _ => None,
            };
        }

        let Event::Key(key) = event else {
            return None;
        };

        match self.surface.on_key(key.code, rows, nav).await {
            KeyOutcome::Event(ReviewSurfaceEvent::CommentSubmitted { anchor, text }) => {
                self.queued_comments.push(ReviewComment::new(anchor, text));
                Some(vec![])
            }
            KeyOutcome::Consumed => Some(vec![]),
            KeyOutcome::PassThrough => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let height = usize::from(ctx.size.height);

        if self.document.lines.is_empty() {
            return Frame::new(vec![Line::new("Plan is empty")]);
        }

        let cursor_anchor = self.current_cursor_anchor();
        self.ensure_rendered(ctx);

        let rendered_plan = &self.cached_surface.get().expect("rendered plan should exist").surface;
        self.surface.restore_cursor(rendered_plan, cursor_anchor);

        self.surface.render_body(rendered_plan, self.queued_comments.iter(), ctx, height)
    }
}

fn build_plan_surface(
    document: &PlanDocument,
    rendered_lines: &[SourceMarkdownLine],
    ctx: &ViewContext,
) -> (AnchoredRows<usize>, AnchorIndex<usize>) {
    let width = ctx.size.width;
    let line_no_width = digit_count(document.line_count().max(1));
    let mut rows = AnchoredSurfaceBuilder::new();
    let mut line_anchors = AnchorIndex::default();

    for rendered in rendered_lines {
        let line_no = rendered.source_line_no;
        let anchor = CommentAnchor(line_no);
        line_anchors.push(anchor);

        let (head, tail) = build_numbered_gutter(line_no, line_no_width, ctx);
        rows.push_anchored_wrapped(anchor, rendered.line.clone(), width, &head, &tail);
    }

    (rows.finish(), line_anchors)
}

fn build_numbered_gutter(line_no: usize, line_no_width: usize, ctx: &ViewContext) -> (Line, Line) {
    let theme = &ctx.theme;
    let mut head = Line::default();
    head.push_with_style(format!("{line_no:>line_no_width$}"), Style::fg(theme.text_secondary()));
    head.push_with_style(" │ ", Style::fg(theme.muted()));
    let tail = Line::new(" ".repeat(line_no_width + 3));
    (head, tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn make_document() -> PlanDocument {
        PlanDocument::parse("/tmp/plan.md", "# Intro\n\nalpha\n\n## Details\n\nbeta")
    }

    #[tokio::test]
    async fn movement_updates_cursor_anchor() {
        let mut panel = PlanPanel::new(make_document());
        let ctx = ViewContext::new((80, 24));
        let _ = panel.render(&ctx);

        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))).await.unwrap();

        assert_eq!(panel.current_source_line_no(), 2);
    }

    #[tokio::test]
    async fn comment_submission_adds_queued_comment() {
        let mut panel = PlanPanel::new(make_document());
        let ctx = ViewContext::new((80, 24));
        let _ = panel.render(&ctx);

        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE))).await.unwrap();
        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE))).await.unwrap();
        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))).await.unwrap();
        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).await.unwrap();

        assert_eq!(panel.comment_count(), 1);
    }

    #[test]
    fn plan_surface_records_source_line_rows_and_insertion_rows() {
        let document = PlanDocument::parse("/tmp/plan.md", &format!("# Intro\n\n{}\n\nshort", "x".repeat(120)));
        let ctx = ViewContext::new((28, 20));
        let result = render_markdown_source_lines(&document.markdown_text(), &ctx);
        let (surface, line_anchors) = build_plan_surface(&document, &result.lines, &ctx);

        assert!(line_anchors.as_slice().contains(&CommentAnchor(3)), "long source line should be anchored");
        let start_row = surface.start_row_for_anchor(CommentAnchor(3)).expect("line should have a start row");
        let end_row = surface.end_row_for_anchor(CommentAnchor(3)).expect("line should have an end row");

        assert!(end_row > start_row, "wrapped source line should span multiple rows");
    }
}
