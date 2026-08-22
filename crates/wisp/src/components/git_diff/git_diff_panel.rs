use super::git_diff_compositor::GitDiffCompositor;
use super::patch_renderer::RenderedPatch;
use super::split_patch_renderer::build_split_patch_base_lines;
use super::{DiffAnchor, PatchAnchor, file_status_color, header_rule, push_diff_stats};
use crate::components::app::git_diff_mode::QueuedComment;
use crate::components::review_comments::{AnchoredRows, KeyOutcome, Navigation, ReviewSurface, ReviewSurfaceEvent};
use crate::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, PatchLineKind, StageState};
use std::collections::HashSet;
use std::path::PathBuf;
use tui::{Component, Cursor, Event, Frame, FramePart, KeyCode, Line, MouseEventKind, Style, ViewContext};

const PAGE_SIZE: usize = 20;
const SCROLL_TRACK_WIDTH: u16 = 1;

/// Which diff view the panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffViewMode {
    /// Only the diff hunks with standard context (default).
    HunkDiff,
    /// The entire file content with diff annotations.
    FileView,
}

/// State for the full-file view, which renders the working-tree file with diff
/// annotations. Extracted so it can be cleared as a unit when switching files.
struct FileView {
    split_layout: bool,
    repo_root: PathBuf,
    patch: Option<RenderedPatch>,
    path: String,
    added_line_numbers: HashSet<usize>,
    file_content: Option<String>,
}

impl FileView {
    fn new(repo_root: PathBuf) -> Self {
        Self {
            split_layout: false,
            repo_root,
            patch: None,
            path: String::new(),
            added_line_numbers: HashSet::new(),
            file_content: None,
        }
    }

    fn set_file(&mut self, file: &FileDiff) -> bool {
        if self.path == file.path {
            return false;
        }
        self.path.clone_from(&file.path);
        self.added_line_numbers = file
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == PatchLineKind::Added)
            .filter_map(|line| line.new_line_no)
            .collect();
        self.patch = None;
        self.file_content = None;
        true
    }

    fn clear_patch(&mut self) {
        self.patch = None;
    }

    fn load_file_content(&mut self) {
        if self.file_content.is_some() || self.path.is_empty() {
            return;
        }
        let full_path = self.repo_root.join(&self.path);
        self.file_content = std::fs::read_to_string(&full_path).ok();
    }

    fn ensure_patch(&mut self, width: usize, file_status: FileStatus, ctx: &ViewContext) {
        if self.patch.is_some() || self.path.is_empty() {
            return;
        }

        let synthetic = self.build_synthetic_diff(file_status);
        let patch = if self.split_layout {
            build_split_patch_base_lines(&synthetic, width, ctx)
        } else {
            RenderedPatch::from_file_diff(&synthetic, width, ctx)
        };
        self.patch = Some(patch);
    }

    fn rendered_patch(&self) -> Option<&RenderedPatch> {
        self.patch.as_ref()
    }

    fn build_synthetic_diff(&self, file_status: FileStatus) -> FileDiff {
        let Some(content) = &self.file_content else {
            return FileDiff {
                old_path: None,
                path: self.path.clone(),
                status: file_status,
                staged: StageState::Unstaged,
                hunks: Vec::new(),
                binary: false,
            };
        };

        let file_lines: Vec<&str> = content.lines().collect();

        let synthetic_lines: Vec<PatchLine> = file_lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let new_line_no = i + 1;
                if self.added_line_numbers.contains(&new_line_no) {
                    PatchLine::added(*text, new_line_no)
                } else {
                    PatchLine::context(*text, new_line_no, new_line_no)
                }
            })
            .collect();

        FileDiff {
            old_path: None,
            path: self.path.clone(),
            status: file_status,
            staged: StageState::Unstaged,
            hunks: vec![Hunk {
                header: format!("@@ -0,0 +1,{} @@", file_lines.len()),
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: file_lines.len(),
                lines: synthetic_lines,
            }],
            binary: false,
        }
    }
}

pub struct GitDiffPanel {
    file_header: String,
    file_status: FileStatus,
    file_additions: usize,
    file_deletions: usize,
    binary: bool,
    header_left_padding: usize,
    focused: bool,
    saved_cursor_anchor: Option<DiffAnchor>,
    surface: ReviewSurface<PatchAnchor>,
    compositor: GitDiffCompositor,
    view_mode: DiffViewMode,
    file_view: FileView,
}

pub enum GitDiffPanelMessage {
    CommentSubmitted { anchor: DiffAnchor, text: String },
}

impl GitDiffPanel {
    pub fn new() -> Self {
        Self {
            file_header: String::new(),
            file_status: FileStatus::Modified,
            file_additions: 0,
            file_deletions: 0,
            binary: false,
            header_left_padding: 0,
            focused: false,
            saved_cursor_anchor: None,
            surface: ReviewSurface::new(),
            compositor: GitDiffCompositor::new(),
            view_mode: DiffViewMode::HunkDiff,
            file_view: FileView::new(PathBuf::new()),
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn set_repo_root(&mut self, root: PathBuf) {
        self.file_view.repo_root = root;
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
        self.set_file(file);

        let cursor_anchor = self.saved_cursor_anchor.take();
        self.update_file_header(file);

        if file.binary {
            self.compositor.deactivate_rendered_patch();
            self.restore_cursor_to_anchor(cursor_anchor);
            return;
        }

        self.ensure_rendered_layers(file, comments, width, document_revision);
        self.restore_cursor_to_anchor(cursor_anchor);
    }

    fn set_file(&mut self, file: &FileDiff) {
        if self.file_view.set_file(file) {
            self.view_mode = DiffViewMode::HunkDiff;
        }
    }

    fn ensure_rendered_layers(
        &mut self,
        file: &FileDiff,
        comments: &[&QueuedComment],
        width: u16,
        document_revision: usize,
    ) {
        let content_width = width.saturating_sub(SCROLL_TRACK_WIDTH);
        let has_removals =
            file.hunks.iter().flat_map(|hunk| &hunk.lines).any(|line| line.kind == PatchLineKind::Removed);
        let use_split_patch = content_width >= tui::MIN_SPLIT_WIDTH && has_removals;
        self.header_left_padding = if use_split_patch { tui::header_left_padding(file.max_line_no()) } else { 0 };

        if self.file_view.split_layout != use_split_patch {
            self.file_view.split_layout = use_split_patch;
            self.file_view.clear_patch();
        }

        let context = ViewContext::new((content_width, 0));
        self.compositor.ensure_diff_layer(file, content_width, use_split_patch, document_revision, &context);
        self.compositor.ensure_submitted_layer(file, comments, &context);
    }

    /// Resolves the [`AnchoredRows`] backing the active view. Takes the relevant
    /// fields by reference (rather than `&self`) so callers can still mutably
    /// borrow the disjoint `surface` cursor in the same expression.
    fn pick_surface<'a>(
        view_mode: DiffViewMode,
        compositor: &'a GitDiffCompositor,
        file_view: &'a FileView,
    ) -> Option<&'a AnchoredRows<PatchAnchor>> {
        let patch = match view_mode {
            DiffViewMode::HunkDiff => compositor.rendered_patch(),
            DiffViewMode::FileView => file_view.rendered_patch(),
        };
        patch.map(|patch| &patch.surface)
    }

    fn active_surface(&self) -> Option<&AnchoredRows<PatchAnchor>> {
        Self::pick_surface(self.view_mode, &self.compositor, &self.file_view)
    }

    fn current_cursor_anchor(&self) -> Option<DiffAnchor> {
        self.active_surface().and_then(|surface| surface.anchor_at_or_before(self.surface.cursor().row))
    }

    fn restore_cursor_to_anchor(&mut self, anchor: Option<DiffAnchor>) {
        match Self::pick_surface(self.view_mode, &self.compositor, &self.file_view) {
            Some(surface) => surface.restore_cursor(self.surface.cursor_mut(), anchor),
            None => self.surface.cursor_mut().row = 0,
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
        self.file_additions = file.additions();
        self.file_deletions = file.deletions();
        self.binary = file.binary;
        self.header_left_padding = 0;
    }

    fn render_header_line(&self, theme: &tui::Theme) -> Line {
        let status_label = self.file_status.label();
        let status_color = file_status_color(self.file_status, theme);

        let mut line = Line::default();
        if self.header_left_padding > 0 {
            line.push_text(" ".repeat(self.header_left_padding));
        }
        let (dir, base) = match self.file_header.rsplit_once('/') {
            Some((dir, base)) => (format!("{dir}/"), base),
            None => (String::new(), self.file_header.as_str()),
        };
        if !dir.is_empty() {
            line.push_with_style(dir, Style::fg(theme.muted()));
        }
        let base_style = if self.focused { Style::fg(theme.accent()).bold() } else { Style::default().bold() };
        line.push_with_style(base, base_style);
        line.push_with_style(format!("  {status_label}"), Style::fg(status_color));
        if !self.binary {
            line.push_text("  ");
            push_diff_stats(&mut line, self.file_additions, self.file_deletions, theme);
        }

        line
    }

    fn render_binary_frame(&self, theme: &tui::Theme, width: usize, height: usize) -> Frame {
        let mut lines = Vec::with_capacity(height);

        for row in 0..height {
            let mut line = Line::default();
            if row == 0 {
                line = self.render_header_line(theme);
            } else if row == 1 {
                line = header_rule(width, theme);
            } else if row == 2 {
                line.push_with_style("Binary file", Style::fg(theme.text_secondary()));
            }
            lines.push(line);
        }

        Frame::new(lines).with_cursor(Cursor::hidden())
    }

    fn render_scroll_track(&self, total_lines: usize, body_height: usize, theme: &tui::Theme) -> Frame {
        if body_height == 0 || total_lines <= body_height {
            return Frame::new(vec![Line::new(" "); body_height.max(1)]);
        }

        let thumb_len = (body_height * body_height / total_lines).clamp(1, body_height);
        let max_scroll = total_lines - body_height;
        let scroll = self.surface.cursor().scroll.min(max_scroll);
        let thumb_top = scroll * (body_height - thumb_len) / max_scroll.max(1);

        let mut rows = vec![("│", Style::fg(theme.muted()).dim()); body_height];
        for row in rows.iter_mut().skip(thumb_top).take(thumb_len) {
            *row = ("█", Style::fg(theme.text_secondary()));
        }

        Frame::new(rows.into_iter().map(|(glyph, style)| Line::with_style(glyph, style)).collect())
    }

    fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            DiffViewMode::HunkDiff => {
                self.file_view.load_file_content();
                DiffViewMode::FileView
            }
            DiffViewMode::FileView => DiffViewMode::HunkDiff,
        };
        self.surface.cursor_mut().row = 0;
        self.surface.cursor_mut().scroll = 0;
    }
}

impl Component for GitDiffPanel {
    type Message = GitDiffPanelMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Key(key) = event
            && key.code == KeyCode::Char('o')
            && !self.is_in_comment_mode()
        {
            self.toggle_view_mode();
            return Some(vec![]);
        }

        let surface = Self::pick_surface(self.view_mode, &self.compositor, &self.file_view)?;

        if let Event::Mouse(mouse) = event {
            return match mouse.kind {
                MouseEventKind::ScrollUp if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(-3, surface, Navigation::RowStep { page_size: PAGE_SIZE });
                    Some(vec![])
                }
                MouseEventKind::ScrollDown if !self.is_in_comment_mode() => {
                    self.surface.on_mouse_scroll(3, surface, Navigation::RowStep { page_size: PAGE_SIZE });
                    Some(vec![])
                }
                _ => None,
            };
        }

        let Event::Key(key) = event else {
            return None;
        };

        let outcome = self.surface.on_key(key.code, surface, Navigation::RowStep { page_size: PAGE_SIZE }).await;
        match outcome {
            KeyOutcome::Event(ReviewSurfaceEvent::CommentSubmitted { anchor, text }) => {
                Some(vec![GitDiffPanelMessage::CommentSubmitted { anchor, text }])
            }
            KeyOutcome::Consumed => Some(vec![]),
            KeyOutcome::PassThrough => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let theme = &ctx.theme;
        let width = usize::from(ctx.size.width);
        let height = usize::from(ctx.size.height);

        if self.binary {
            return self.render_binary_frame(theme, width, height);
        }

        let body_height = height.saturating_sub(2);

        if self.view_mode == DiffViewMode::FileView {
            let content_width = width.saturating_sub(usize::from(SCROLL_TRACK_WIDTH));
            self.file_view.ensure_patch(content_width, self.file_status, ctx);
        }

        let rendered_surface = Self::pick_surface(self.view_mode, &self.compositor, &self.file_view)
            .filter(|surface| !surface.lines().is_empty());

        let Some(rendered_surface) = rendered_surface else {
            let mut lines = vec![self.render_header_line(theme)];
            lines.resize(height, Line::default());
            return Frame::new(lines).with_cursor(Cursor::hidden());
        };

        let comment_splices = self.compositor.comment_splices();
        let content_width = tui::usize_to_u16_saturating(width).saturating_sub(SCROLL_TRACK_WIDTH);
        let content_ctx = ctx.with_size((content_width, ctx.size.height));
        let viewport =
            self.surface.render_body_with_splices(rendered_surface, comment_splices, &content_ctx, body_height);

        let splice_height: usize = comment_splices.iter().map(|splice| splice.frame.lines().len()).sum();
        let total_lines = rendered_surface.lines().len() + splice_height;
        let track = self.render_scroll_track(total_lines, body_height, theme);
        let body = Frame::hstack([FramePart::new(viewport, content_width), FramePart::new(track, SCROLL_TRACK_WIDTH)]);

        let mut header_lines = vec![self.render_header_line(theme)];
        if height > 1 {
            header_lines.push(header_rule(width, theme));
        }

        Frame::vstack([Frame::new(header_lines), body])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn modified_file() -> FileDiff {
        FileDiff {
            old_path: None,
            path: "src/main.rs".to_string(),
            status: FileStatus::Modified,
            staged: StageState::Unstaged,
            hunks: vec![Hunk {
                header: "@@ -1,1 +1,2 @@".to_string(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 2,
                lines: vec![PatchLine::context("fn main() {", 1, 1), PatchLine::added("    let value = 1;", 2)],
            }],
            binary: false,
        }
    }

    /// Pressing `o` toggles the full-file view, but only outside comment mode:
    /// while composing a comment it must reach the draft as literal text.
    #[tokio::test]
    async fn typing_o_while_commenting_inserts_the_letter() {
        let mut panel = GitDiffPanel::new();
        panel.ensure_layers(&modified_file(), &[], 100, 0);

        panel.on_event(&key(KeyCode::Char('c'))).await;
        assert!(panel.is_in_comment_mode(), "pressing 'c' should open a draft comment");

        for ch in "good".chars() {
            panel.on_event(&key(KeyCode::Char(ch))).await;
        }
        let messages = panel.on_event(&key(KeyCode::Enter)).await.expect("submitting should emit a message");

        let text = messages
            .into_iter()
            .map(|GitDiffPanelMessage::CommentSubmitted { text, .. }| text)
            .next()
            .expect("expected a CommentSubmitted message");
        assert_eq!(text, "good", "every typed character, including 'o', must reach the draft");
    }
}
