use super::patch_renderer::RenderedPatch;
use super::split_patch_renderer::build_split_patch_base_lines;
use crate::components::app::git_diff_mode::QueuedComment;
use crate::components::common::CachedLayer;
use crate::components::review_comments::{CommentGroup, FrameSplice};
use crate::git_diff::FileDiff;
use tui::ViewContext;

const MAX_CACHED_DIFF_LAYERS: usize = 16;

#[derive(Clone, PartialEq, Eq)]
struct DiffLayerKey {
    document_revision: usize,
    width: u16,
    file_path: String,
    layout: DiffLayout,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffLayout {
    Unified,
    Split,
}

#[derive(Clone, PartialEq, Eq)]
struct CommentLayerKey {
    revision: usize,
    width: u16,
    file_path: String,
}

struct CachedRenderedPatch {
    key: DiffLayerKey,
    patch: RenderedPatch,
}

pub struct GitDiffCompositor {
    rendered_patches: Vec<CachedRenderedPatch>,
    has_active_rendered_patch: bool,
    submitted_layer: CachedLayer<CommentLayerKey, Vec<FrameSplice>>,
    submitted_revision: usize,
}

impl GitDiffCompositor {
    pub fn new() -> Self {
        Self {
            rendered_patches: Vec::new(),
            has_active_rendered_patch: false,
            submitted_layer: CachedLayer::new(),
            submitted_revision: 0,
        }
    }

    pub fn clear_rendered_patches(&mut self) {
        self.rendered_patches.clear();
        self.has_active_rendered_patch = false;
        self.submitted_layer.reset();
    }

    pub fn deactivate_rendered_patch(&mut self) {
        self.has_active_rendered_patch = false;
        self.submitted_layer.reset();
    }

    pub fn invalidate_comment_splices(&mut self) {
        self.submitted_revision = self.submitted_revision.saturating_add(1);
        self.submitted_layer.reset();
    }

    pub fn ensure_diff_layer(
        &mut self,
        file: &FileDiff,
        width: u16,
        split_layout: bool,
        document_revision: usize,
        ctx: &ViewContext,
    ) {
        let key = DiffLayerKey {
            document_revision,
            width,
            file_path: file.path.clone(),
            layout: if split_layout { DiffLayout::Split } else { DiffLayout::Unified },
        };

        if let Some(index) = self.rendered_patches.iter().position(|cached| cached.key == key) {
            let cached = self.rendered_patches.remove(index);
            self.rendered_patches.push(cached);
            self.has_active_rendered_patch = true;
            return;
        }

        let patch = if split_layout {
            build_split_patch_base_lines(file, usize::from(width), ctx)
        } else {
            RenderedPatch::from_file_diff(file, usize::from(width), ctx)
        };

        if self.rendered_patches.len() == MAX_CACHED_DIFF_LAYERS {
            self.rendered_patches.remove(0);
        }

        self.rendered_patches.push(CachedRenderedPatch { key, patch });
        self.has_active_rendered_patch = true;
    }

    pub fn ensure_submitted_layer(&mut self, file: &FileDiff, comments: &[&QueuedComment], ctx: &ViewContext) {
        let rendered = if self.has_active_rendered_patch {
            self.rendered_patches.last().map(|cached| &cached.patch)
        } else {
            None
        };
        let Some(rendered) = rendered else {
            self.submitted_layer.reset();
            return;
        };

        let key =
            CommentLayerKey { revision: self.submitted_revision, width: ctx.size.width, file_path: file.path.clone() };
        self.submitted_layer.ensure(key, || {
            CommentGroup::splices_for(&rendered.surface, comments.iter().map(|comment| &comment.review), ctx)
        });
    }

    pub fn rendered_patch(&self) -> Option<&RenderedPatch> {
        if self.has_active_rendered_patch { self.rendered_patches.last().map(|cached| &cached.patch) } else { None }
    }

    pub fn comment_splices(&self) -> &[FrameSplice] {
        self.submitted_layer.get().map_or(&[], Vec::as_slice)
    }
}

impl Default for GitDiffCompositor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::app::git_diff_mode::GitDiffCommentContext;
    use crate::components::git_diff::PatchAnchor;
    use crate::components::review_comments::{CommentAnchor, ReviewComment};
    use crate::git_diff::{FileStatus, Hunk, PatchLine, PatchLineKind};

    fn context() -> ViewContext {
        ViewContext::new((120, 24))
    }

    fn make_file(lines: Vec<PatchLine>) -> FileDiff {
        make_file_with_path("test.rs", lines)
    }

    fn make_file_with_path(path: &str, lines: Vec<PatchLine>) -> FileDiff {
        FileDiff {
            old_path: Some(path.to_string()),
            path: path.to_string(),
            status: FileStatus::Modified,
            hunks: vec![Hunk {
                header: "@@ -1,1 +1,1 @@".to_string(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines,
            }],
            binary: false,
        }
    }

    fn queued(anchor: CommentAnchor<PatchAnchor>, comment: &str) -> QueuedComment {
        QueuedComment {
            review: ReviewComment::new(anchor, comment),
            context: GitDiffCommentContext {
                file_path: "test.rs".to_string(),
                line_text: "line".to_string(),
                line_number: Some(1),
                line_kind: PatchLineKind::Added,
            },
        }
    }

    fn initialize_layers(
        compositor: &mut GitDiffCompositor,
        file: &FileDiff,
        width: u16,
        comments: &[QueuedComment],
        document_revision: usize,
    ) {
        let ctx = context().with_width(width);
        let refs = comments.iter().collect::<Vec<_>>();
        compositor.ensure_diff_layer(file, width, false, document_revision, &ctx);
        compositor.ensure_submitted_layer(file, &refs, &ctx);
    }

    fn added_line(text: &str) -> PatchLine {
        PatchLine { kind: PatchLineKind::Added, text: text.to_string(), old_line_no: None, new_line_no: Some(1) }
    }

    fn rendered_patch_contains(compositor: &GitDiffCompositor, marker: &str) -> bool {
        compositor
            .rendered_patch()
            .expect("rendered patch should exist")
            .surface
            .lines()
            .iter()
            .any(|line| line.plain_text().contains(marker))
    }

    #[test]
    fn switching_back_to_cached_file_updates_active_patch() {
        let file_a = make_file_with_path("a.rs", vec![added_line("file a marker")]);
        let file_b = make_file_with_path("b.rs", vec![added_line("file b marker")]);
        let ctx = context().with_width(80);
        let mut compositor = GitDiffCompositor::new();

        compositor.ensure_diff_layer(&file_a, 80, false, 1, &ctx);
        compositor.ensure_diff_layer(&file_b, 80, false, 1, &ctx);
        compositor.ensure_diff_layer(&file_a, 80, false, 1, &ctx);

        assert!(rendered_patch_contains(&compositor, "file a marker"));
        assert!(!rendered_patch_contains(&compositor, "file b marker"));
    }

    #[test]
    fn clear_rendered_patches_clears_cached_patches() {
        let file = make_file(vec![added_line("cached marker")]);
        let ctx = context().with_width(80);
        let mut compositor = GitDiffCompositor::new();

        compositor.ensure_diff_layer(&file, 80, false, 1, &ctx);
        compositor.clear_rendered_patches();

        assert!(compositor.rendered_patch().is_none());
        compositor.ensure_diff_layer(&file, 80, false, 1, &ctx);
        assert!(rendered_patch_contains(&compositor, "cached marker"));
    }

    #[test]
    fn rendered_patch_tracks_active_file() {
        let file_a = make_file_with_path("a.rs", vec![added_line("file a marker")]);
        let file_b = make_file_with_path("b.rs", vec![added_line("file b marker")]);
        let ctx = context().with_width(80);
        let mut compositor = GitDiffCompositor::new();

        compositor.ensure_diff_layer(&file_a, 80, false, 1, &ctx);
        assert!(rendered_patch_contains(&compositor, "file a marker"));
        assert!(!rendered_patch_contains(&compositor, "file b marker"));

        compositor.ensure_diff_layer(&file_b, 80, false, 1, &ctx);
        assert!(rendered_patch_contains(&compositor, "file b marker"));
        assert!(!rendered_patch_contains(&compositor, "file a marker"));
    }

    #[test]
    fn compositor_starts_empty() {
        let compositor = GitDiffCompositor::new();
        assert!(compositor.rendered_patch().is_none());
        assert!(compositor.comment_splices().is_empty());
    }

    #[test]
    fn comment_splices_preserve_input_order_for_same_anchor() {
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine {
                kind: PatchLineKind::Added,
                text: "new_line();".to_string(),
                old_line_no: None,
                new_line_no: Some(1),
            },
        ]);

        let anchor = CommentAnchor(PatchAnchor { hunk: 0, line: 1 });
        let comments = vec![queued(anchor, "alpha"), queued(anchor, "beta")];

        let mut compositor = GitDiffCompositor::new();
        initialize_layers(&mut compositor, &file, 80, &comments, 1);

        let splices = compositor.comment_splices();
        assert_eq!(splices.len(), 1);
        let rendered_text: Vec<String> = splices[0].frame.lines().iter().map(tui::Line::plain_text).collect();
        let alpha_pos = rendered_text.iter().position(|t| t.contains("alpha")).expect("alpha should render");
        let beta_pos = rendered_text.iter().position(|t| t.contains("beta")).expect("beta should render");
        assert!(alpha_pos < beta_pos, "comments should render in queue order");
    }

    #[test]
    fn comment_splice_uses_correct_after_row() {
        let file = make_file(vec![
            PatchLine {
                kind: PatchLineKind::HunkHeader,
                text: "@@ -1,1 +1,1 @@".to_string(),
                old_line_no: None,
                new_line_no: None,
            },
            PatchLine {
                kind: PatchLineKind::Added,
                text: "new_line();".to_string(),
                old_line_no: None,
                new_line_no: Some(1),
            },
        ]);

        let anchor = CommentAnchor(PatchAnchor { hunk: 0, line: 1 });
        let comments = vec![queued(anchor, "a comment")];

        let mut compositor = GitDiffCompositor::new();
        initialize_layers(&mut compositor, &file, 80, &comments, 1);

        let splices = compositor.comment_splices();
        assert_eq!(splices.len(), 1);

        let rendered = compositor.rendered_patch().unwrap();
        let end_row = rendered.surface.end_row_for_anchor(anchor).expect("anchor end row should exist");
        assert_eq!(splices[0].after_row, end_row);
    }

    #[test]
    fn deactivate_rendered_patch_clears_active_but_keeps_cache() {
        let file = make_file(vec![added_line("deactivate marker")]);
        let ctx = context().with_width(80);
        let mut compositor = GitDiffCompositor::new();

        compositor.ensure_diff_layer(&file, 80, false, 1, &ctx);
        assert!(compositor.rendered_patch().is_some());

        compositor.deactivate_rendered_patch();
        assert!(compositor.rendered_patch().is_none());

        compositor.ensure_diff_layer(&file, 80, false, 1, &ctx);
        assert!(rendered_patch_contains(&compositor, "deactivate marker"));
    }
}
