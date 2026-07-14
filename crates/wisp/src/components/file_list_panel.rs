use crate::components::common::VerticalCursor;
use crate::components::file_tree::{FileTree, FileTreeEntry, FileTreeEntryKind};
use crate::components::git_diff::{file_status_color, header_rule, push_diff_stats};
use crate::git_diff::{DiffScope, FileDiff, FileStatus, StageState};
use tui::{Component, Event, Frame, KeyCode, Line, MouseEventKind, Style, ViewContext, truncate_line, truncate_text};

const CHROME_HEIGHT: usize = 2;

pub struct FileListPanel {
    tree: FileTree,
    cursor: VerticalCursor,
    queued_comment_count: usize,
    file_count: usize,
    additions: usize,
    deletions: usize,
    focused: bool,
    file_comment_counts: Vec<usize>,
    diff_scope: DiffScope,
    scroll_consumed_this_frame: bool,
}

pub enum FileListMessage {
    Selected(usize),
    FileOpened(usize),
}

impl Default for FileListPanel {
    fn default() -> Self {
        Self {
            tree: FileTree::empty(),
            cursor: VerticalCursor::new(),
            queued_comment_count: 0,
            file_count: 0,
            additions: 0,
            deletions: 0,
            focused: false,
            file_comment_counts: Vec::new(),
            diff_scope: DiffScope::default(),
            scroll_consumed_this_frame: false,
        }
    }
}

impl FileListPanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rebuild_from_files(&mut self, files: &[FileDiff]) {
        self.file_count = files.len();
        self.additions = files.iter().map(FileDiff::additions).sum();
        self.deletions = files.iter().map(FileDiff::deletions).sum();
        self.tree = FileTree::from_files(files);
        self.cursor = VerticalCursor::new();
        self.file_comment_counts = vec![0; files.len()];
    }

    pub fn selected_file_index(&self) -> Option<usize> {
        self.tree.selected_file_index()
    }

    pub fn select_file_index(&mut self, file_index: usize) {
        self.tree.select_file_index(file_index);
    }

    pub fn sync_view_state(&mut self, queued_comment_count: usize, file_comment_counts: Vec<usize>) {
        self.queued_comment_count = queued_comment_count;
        self.file_comment_counts = file_comment_counts;
    }

    pub fn set_diff_scope(&mut self, diff_scope: DiffScope) {
        self.diff_scope = diff_scope;
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub(crate) fn select_relative(&mut self, delta: isize) -> Option<usize> {
        let prev_file = self.tree.selected_file_index();
        self.tree.navigate(delta);
        let new_file = self.tree.selected_file_index();
        if let Some(idx) = new_file
            && Some(idx) != prev_file
        {
            return Some(idx);
        }
        None
    }

    pub(crate) fn tree_collapse_or_parent(&mut self) {
        self.tree.collapse_or_parent();
    }

    pub(crate) fn tree_expand_or_enter(&mut self) -> Option<usize> {
        let is_file = self.tree.expand_or_enter();
        if is_file { self.tree.selected_file_index() } else { None }
    }

    pub fn tree_mut(&mut self) -> &mut FileTree {
        &mut self.tree
    }

    fn header_row(&self, width: usize, theme: &tui::Theme) -> Line {
        let title_fg = if self.focused { theme.accent() } else { theme.text_primary() };
        let mut row = Line::default();
        row.push_text(" ");
        row.push_with_style(format!("Git Diff · {}", self.diff_scope.label()), Style::fg(title_fg).bold());
        row.push_text("  ");
        row.push_with_style(
            format!("{} file{}", self.file_count, if self.file_count == 1 { "" } else { "s" }),
            Style::fg(theme.text_primary()),
        );
        row.push_text("  ");
        push_diff_stats(&mut row, self.additions, self.deletions, theme);
        if self.queued_comment_count > 0 {
            row.push_text("  ");
            row.push_with_style(format!("◆{}", self.queued_comment_count), Style::fg(theme.accent()));
        }
        row.extend_bg_to_width(width);
        truncate_line(&row, width)
    }

    fn ensure_visible(&mut self, viewport_height: usize) {
        self.cursor.ensure_visible(self.tree.selected_visible(), viewport_height);
    }
}

impl Component for FileListPanel {
    type Message = FileListMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Mouse(mouse) = event {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if self.scroll_consumed_this_frame {
                        return Some(vec![]);
                    }
                    self.scroll_consumed_this_frame = true;
                    Some(self.select_relative(-1).map(|idx| vec![FileListMessage::Selected(idx)]).unwrap_or_default())
                }
                MouseEventKind::ScrollDown => {
                    if self.scroll_consumed_this_frame {
                        return Some(vec![]);
                    }
                    self.scroll_consumed_this_frame = true;
                    Some(self.select_relative(1).map(|idx| vec![FileListMessage::Selected(idx)]).unwrap_or_default())
                }
                _ => None,
            };
        }

        let Event::Key(key) = event else {
            return None;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                Some(self.select_relative(1).map(|idx| vec![FileListMessage::Selected(idx)]).unwrap_or_default())
            }
            KeyCode::Char('k') | KeyCode::Up => {
                Some(self.select_relative(-1).map(|idx| vec![FileListMessage::Selected(idx)]).unwrap_or_default())
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.tree_collapse_or_parent();
                Some(vec![])
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if let Some(idx) = self.tree_expand_or_enter() {
                    Some(vec![FileListMessage::FileOpened(idx)])
                } else {
                    Some(vec![])
                }
            }
            _ => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let theme = &ctx.theme;
        self.scroll_consumed_this_frame = false;
        let width = ctx.size.width as usize;
        let height = ctx.size.height as usize;
        if width < 2 {
            return Frame::new((0..height).map(|_| Line::new(" ".repeat(width))).collect());
        }

        let tree_height = height.saturating_sub(CHROME_HEIGHT);
        self.ensure_visible(tree_height);

        let mut lines = Vec::with_capacity(height);
        lines.push(self.header_row(width, theme));
        lines.push(header_rule(width, theme));

        let visible_entries = self.tree.visible_entries();
        let tree_selected = self.tree.selected_visible();
        for row in 0..tree_height {
            let entry_index = row + self.cursor.scroll;
            let mut content = Line::default();
            if let Some(entry) = visible_entries.get(entry_index) {
                let is_selected = entry_index == tree_selected;
                let comments =
                    entry_file_index(entry).and_then(|index| self.file_comment_counts.get(index).copied()).unwrap_or(0);
                let indent = tree_indent(visible_entries, entry_index);
                let flags = EntryFlags { is_selected, indent: &indent, width, comments };
                match &entry.kind {
                    FileTreeEntryKind::Directory { name, expanded, .. } => {
                        render_directory_entry(&mut content, name, *expanded, entry.depth, flags, theme);
                    }
                    FileTreeEntryKind::File { name, status, staged, additions, deletions, .. } => {
                        let row = FileRow {
                            name,
                            status: *status,
                            staged: *staged,
                            additions: *additions,
                            deletions: *deletions,
                        };
                        render_file_entry(&mut content, row, flags, theme);
                    }
                }
            } else {
                content.push_text(" ".repeat(width));
            }
            lines.push(content);
        }

        lines.truncate(height);
        Frame::new(lines)
    }
}

fn entry_file_index(entry: &FileTreeEntry) -> Option<usize> {
    match &entry.kind {
        FileTreeEntryKind::File { file_index, .. } => Some(*file_index),
        FileTreeEntryKind::Directory { .. } => None,
    }
}

#[derive(Clone, Copy)]
struct EntryFlags<'a> {
    is_selected: bool,
    indent: &'a str,
    width: usize,
    comments: usize,
}

fn render_directory_entry(
    line: &mut Line,
    name: &str,
    expanded: bool,
    depth: usize,
    flags: EntryFlags<'_>,
    theme: &tui::Theme,
) {
    let EntryFlags { is_selected, indent, width, .. } = flags;
    let icon = if expanded { "▾" } else { "▸" };
    let connector = if depth == 0 { "" } else { "─" };
    let icon_gap = if depth == 0 { "  " } else { " " };
    let dir_style = row_fg_style(theme.info(), is_selected, theme);
    let indicator = if is_selected { "▎" } else { " " };
    let prefix_width = format!("{indicator}{indent}{connector}{icon}{icon_gap}").chars().count();
    let name_budget = width.saturating_sub(prefix_width);
    let display_name = format!("{name}/");
    let truncated = truncate_text(&display_name, name_budget);

    line.push_with_style(indicator, row_fg_style(theme.accent(), is_selected, theme));
    line.push_with_style(format!("{indent}{connector}"), row_fg_style(theme.muted(), is_selected, theme));
    line.push_with_style(format!("{icon}{icon_gap}"), dir_style);
    line.push_with_style(truncated.as_ref(), dir_style.bold());
    line.extend_bg_to_width(width);
}

#[derive(Clone, Copy)]
struct FileRow<'a> {
    name: &'a str,
    status: FileStatus,
    staged: StageState,
    additions: usize,
    deletions: usize,
}

fn render_file_entry(line: &mut Line, row: FileRow<'_>, flags: EntryFlags<'_>, theme: &tui::Theme) {
    let FileRow { name, status, staged, additions, deletions } = row;
    let EntryFlags { is_selected, indent, width, comments } = flags;
    let style = row_style(is_selected, theme);
    let guide_style = row_fg_style(theme.muted(), is_selected, theme);
    let indicator = if is_selected { "▎" } else { " " };
    let (checkbox, checkbox_color) = stage_checkbox(staged, theme);

    let badge = (comments > 0).then(|| format!("◆{comments} "));
    let badge_width = badge.as_deref().map_or(0, |b| b.chars().count());
    let add_str = format!("+{additions}");
    let del_str = format!(" -{deletions}");
    let marker_str = format!(" {}", status.marker());
    let checkbox_str = format!(" {checkbox}");
    let suffix_width = badge_width
        + add_str.chars().count()
        + del_str.chars().count()
        + marker_str.chars().count()
        + checkbox_str.chars().count();
    let prefix = format!("{indicator}{indent}── ");
    let prefix_width = prefix.chars().count();
    let name_budget = width.saturating_sub(prefix_width + suffix_width + 1);
    let truncated = truncate_text(name, name_budget);

    line.push_with_style(indicator, row_fg_style(theme.accent(), is_selected, theme));
    line.push_with_style(format!("{indent}── "), guide_style);
    line.push_with_style(truncated.as_ref(), style);
    let padding = width.saturating_sub(prefix_width + truncated.chars().count() + suffix_width);
    if padding > 0 {
        line.push_with_style(" ".repeat(padding), style);
    }
    if let Some(badge) = badge {
        line.push_with_style(badge, row_fg_style(theme.accent(), is_selected, theme));
    }
    line.push_with_style(add_str, row_fg_style(theme.diff_added_fg(), is_selected, theme));
    line.push_with_style(del_str, row_fg_style(theme.diff_removed_fg(), is_selected, theme));
    line.push_with_style(marker_str, row_fg_style(file_status_color(status, theme), is_selected, theme));
    line.push_with_style(checkbox_str, row_fg_style(checkbox_color, is_selected, theme));
}

fn stage_checkbox(staged: StageState, theme: &tui::Theme) -> (&'static str, tui::Color) {
    match staged {
        StageState::Staged => ("☑", theme.diff_added_fg()),
        StageState::PartiallyStaged => ("▣", theme.warning()),
        StageState::Unstaged => ("☐", theme.muted()),
    }
}

fn row_style(is_selected: bool, theme: &tui::Theme) -> Style {
    if is_selected { theme.selected_row_style() } else { Style::default() }
}

fn row_fg_style(fg: tui::Color, is_selected: bool, theme: &tui::Theme) -> Style {
    if is_selected { theme.selected_row_style_with_fg(fg) } else { Style::fg(fg) }
}

fn tree_indent(entries: &[FileTreeEntry], index: usize) -> String {
    let Some(entry) = entries.get(index) else {
        return String::new();
    };
    if entry.depth == 0 {
        return String::new();
    }
    let mut indent = String::new();
    for level in 1..entry.depth {
        indent.push_str(if level_continues(entries, index, level) { "│ " } else { "  " });
    }
    indent.push(if level_continues(entries, index, entry.depth) { '├' } else { '└' });
    indent
}

fn level_continues(entries: &[FileTreeEntry], index: usize, level: usize) -> bool {
    for next in &entries[index + 1..] {
        if next.depth < level {
            return false;
        }
        if next.depth == level {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::Hunk;
    use tui::{KeyEvent, KeyModifiers};

    fn modified(path: &str) -> FileDiff {
        FileDiff {
            old_path: None,
            path: path.to_string(),
            status: FileStatus::Modified,
            staged: StageState::Unstaged,
            hunks: vec![Hunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: Vec::new(),
            }],
            binary: false,
        }
    }

    #[tokio::test]
    async fn active_indicator_follows_selected_directory() {
        let mut panel = FileListPanel::new();
        panel.rebuild_from_files(&[modified("lib/c.rs"), modified("src/a.rs")]);
        panel.select_file_index(0);
        panel.on_event(&Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))).await;

        let frame = panel.render(&ViewContext::new((32, 8)));
        let lines = frame.lines();

        assert!(!lines[3].plain_text().starts_with('▎'), "previous file row should not keep the active indicator");
        assert!(lines[4].plain_text().starts_with('▎'), "selected directory row should have the active indicator");
    }
}
