pub(crate) mod git_diff_compositor;
pub(crate) mod git_diff_panel;
pub(crate) mod patch_renderer;
pub(crate) mod split_patch_renderer;

use crate::components::review_comments::CommentAnchor;
use crate::git_diff::FileStatus;
use tui::{Color, Line, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PatchAnchor {
    pub hunk: usize,
    pub line: usize,
}

pub(crate) type DiffAnchor = CommentAnchor<PatchAnchor>;

pub(crate) fn file_status_color(status: FileStatus, theme: &tui::Theme) -> Color {
    match status {
        FileStatus::Modified => theme.warning(),
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg(),
        FileStatus::Deleted => theme.diff_removed_fg(),
        FileStatus::Renamed => theme.info(),
    }
}

pub(crate) fn push_diff_stats(line: &mut Line, additions: usize, deletions: usize, theme: &tui::Theme) {
    line.push_with_style(format!("+{additions}"), Style::fg(theme.diff_added_fg()));
    line.push_with_style(format!(" -{deletions}"), Style::fg(theme.diff_removed_fg()));
}

pub(crate) fn header_rule(width: usize, theme: &tui::Theme) -> Line {
    Line::with_style("─".repeat(width), Style::fg(theme.muted()))
}
