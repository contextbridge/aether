use super::git_diff_compositor::GitDiffCompositor;
use super::{DiffAnchor, PatchAnchor};
use crate::components::app::git_diff_mode::QueuedComment;
use crate::components::review_comments::{KeyOutcome, Navigation, ReviewSurface, ReviewSurfaceEvent};
use crate::git_diff::{FileDiff, FileStatus, PatchLineKind};
use tui::{Component, Cursor, Event, Frame, KeyCode, Line, MouseEventKind, Style, ViewContext};

const PAGE_SIZE: usize = 20;

pub struct GitDiffPanel {
    file_header: String,
    file_status: FileStatus,
    binary: bool,
    saved_cursor_anchor: Option<DiffAnchor>,
    surface: ReviewSurface<PatchAnchor>,
    compositor: GitDiffCompositor,
}

pub enum GitDiffPanelMessage {
    CommentSubmitted { anchor: DiffAnchor, text: String },
}

impl GitDiffPanel {
    pub fn new() -> Self {
        Self {
            file_header: String::new(),
            file_status: FileStatus::Modified,
            binary: false,
            saved_cursor_anchor: None,
            surface: ReviewSurface::new(),
            compositor: GitDiffCompositor::new(),
        }
    }

    pub fn clear_rendered_patches(&mut self) {
        self.saved_cursor_anchor = self.current_cursor_anchor();
        self.compositor.clear_rendered_patches();
    }

    pub fn invalidate_comment_splices(&mut self) {
        self.compositor.invalidate_comment_splices();
    }

    pub fn reset_for_new_file(&mut self) {
        self.surface = ReviewSurface::new();
        self.saved_cursor_anchor = None;
    }

    pub fn reset_scroll(&mut self) {
        self.surface.cursor_mut().scroll = 0;
    }

    pub fn is_in_comment_mode(&self) -> bool {
        self.surface.is_in_comment_mode()
    }

    pub fn ensure_layers(
        &mut self,
        file: &FileDiff,
        comments: &[&QueuedComment],
        width: u16,
        document_revision: usize,
    ) {
        let cursor_anchor = self.saved_cursor_anchor.take();
        self.update_file_header(file);

        if file.binary {
            self.compositor.deactivate_rendered_patch();
            self.restore_cursor_to_anchor(cursor_anchor);
            return;
        }

        let right_width = usize::from(width);
        let has_removals =
            file.hunks.iter().flat_map(|hunk| &hunk.lines).any(|line| line.kind == PatchLineKind::Removed);
        let use_split_patch = right_width >= 80 && has_removals;

        let context = ViewContext::new((width, 0));
        self.compositor.ensure_diff_layer(file, width, use_split_patch, document_revision, &context);
        self.compositor.ensure_submitted_layer(file, comments, &context);

        self.restore_cursor_to_anchor(cursor_anchor);
    }

    pub(crate) fn jump_next_hunk(&mut self) -> bool {
        let current = self.surface.cursor().row;
        let max = self.max_row();
        let Some(&next) = self.hunk_offsets_slice().iter().find(|&&offset| offset > current) else {
            return false;
        };
        let next = next.min(max);
        let cursor = self.surface.cursor_mut();
        if cursor.row == next {
            return false;
        }
        cursor.row = next;
        true
    }

    pub(crate) fn jump_prev_hunk(&mut self) -> bool {
        let current = self.surface.cursor().row;
        let Some(&prev) = self.hunk_offsets_slice().iter().rev().find(|&&offset| offset < current) else {
            return false;
        };
        let cursor = self.surface.cursor_mut();
        if cursor.row == prev {
            return false;
        }
        cursor.row = prev;
        true
    }

    fn max_row(&self) -> usize {
        self.compositor.rendered_patch().map_or(0, |rendered| rendered.surface.max_row())
    }

    fn current_cursor_anchor(&self) -> Option<DiffAnchor> {
        self.compositor
            .rendered_patch()
            .and_then(|rendered| rendered.surface.anchor_at_or_before(self.surface.cursor().row))
    }

    fn restore_cursor_to_anchor(&mut self, anchor: Option<DiffAnchor>) {
        if let Some(rendered) = self.compositor.rendered_patch() {
            rendered.surface.restore_cursor(self.surface.cursor_mut(), anchor);
        } else {
            self.surface.cursor_mut().row = 0;
        }
    }

    fn update_file_header(&mut self, file: &FileDiff) {
        self.file_header = match file.status {
            FileStatus::Renamed => {
                let old = file.old_path.as_deref().unwrap_or("?");
                format!("{old} -> {}", file.path)
            }
            _ => file.path.clone(),
        };
        self.file_status = file.status;
        self.binary = file.binary;
    }

    fn render_header_line(&self) -> Line {
        let status_label = match self.file_status {
            FileStatus::Modified => "modified",
            FileStatus::Added => "new file",
            FileStatus::Deleted => "deleted",
            FileStatus::Renamed => "renamed",
            FileStatus::Untracked => "untracked",
        };

        let mut line = Line::default();
        line.push_with_style(format!("{}  ({status_label})", self.file_header), Style::default().bold());
        line
    }

    fn render_binary_frame(&self, theme: &tui::Theme, height: usize) -> Frame {
        let mut lines = Vec::with_capacity(height);

        for row in 0..height {
            let mut line = Line::default();
            if row == 0 {
                line = self.render_header_line();
            } else if row == 2 {
                line.push_with_style("Binary file", Style::fg(theme.text_secondary()));
            }
            lines.push(line);
        }

        Frame::new(lines).with_cursor(Cursor::hidden())
    }

    fn hunk_offsets_slice(&self) -> &[usize] {
        self.compositor.rendered_patch().map_or(&[], |rendered| rendered.hunk_offsets.as_slice())
    }
}

impl Component for GitDiffPanel {
    type Message = GitDiffPanelMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Mouse(mouse) = event {
            let rendered = self.compositor.rendered_patch()?;
            return match mouse.kind {
                MouseEventKind::ScrollUp if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(-3, &rendered.surface, Navigation::RowStep { page_size: PAGE_SIZE });
                    Some(vec![])
                }
                MouseEventKind::ScrollDown if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(3, &rendered.surface, Navigation::RowStep { page_size: PAGE_SIZE });
                    Some(vec![])
                }
                _ => None,
            };
        }

        let Event::Key(key) = event else {
            return None;
        };

        let rendered = self.compositor.rendered_patch()?;

        let outcome =
            self.surface.on_key(key.code, &rendered.surface, Navigation::RowStep { page_size: PAGE_SIZE }).await;
        match outcome {
            KeyOutcome::Event(ReviewSurfaceEvent::CommentSubmitted { anchor, text }) => {
                Some(vec![GitDiffPanelMessage::CommentSubmitted { anchor, text }])
            }
            KeyOutcome::Consumed => Some(vec![]),
            KeyOutcome::PassThrough => match key.code {
                KeyCode::Char('n') => {
                    self.jump_next_hunk();
                    Some(vec![])
                }
                KeyCode::Char('p') => {
                    self.jump_prev_hunk();
                    Some(vec![])
                }
                _ => None,
            },
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let theme = &ctx.theme;
        let height = usize::from(ctx.size.height);

        if self.binary {
            return self.render_binary_frame(theme, height);
        }

        let body_height = height.saturating_sub(2);

        let Some(rendered) = self.compositor.rendered_patch() else {
            let mut lines = vec![self.render_header_line()];
            lines.resize(height, Line::default());
            return Frame::new(lines).with_cursor(Cursor::hidden());
        };

        if rendered.surface.lines().is_empty() {
            let mut lines = vec![self.render_header_line()];
            lines.resize(height, Line::default());
            return Frame::new(lines).with_cursor(Cursor::hidden());
        }

        let rendered_surface = &rendered.surface;
        let comment_splices = self.compositor.comment_splices();
        let viewport = self.surface.render_body_with_splices(rendered_surface, comment_splices, ctx, body_height);

        let mut header_lines = vec![self.render_header_line()];
        if height > 1 {
            header_lines.push(Line::default());
        }

        Frame::vstack([Frame::new(header_lines), viewport])
    }
}
