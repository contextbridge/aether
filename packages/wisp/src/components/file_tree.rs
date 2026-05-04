use crate::git_diff::{FileDiff, FileStatus};

pub enum FileTreeNode {
    Directory { name: String, children: Vec<FileTreeNode>, expanded: bool },
    File { file_index: usize, name: String, status: FileStatus, additions: usize, deletions: usize },
}

#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    pub depth: usize,
    pub kind: FileTreeEntryKind,
}

#[derive(Debug, Clone)]
pub enum FileTreeEntryKind {
    Directory { name: String, expanded: bool },
    File { file_index: usize, name: String, status: FileStatus, additions: usize, deletions: usize },
}

pub struct FileTree {
    roots: Vec<FileTreeNode>,
    selected_visible: usize,
    cached_entries: Vec<FileTreeEntry>,
}

impl FileTree {
    pub fn empty() -> Self {
        Self { roots: Vec::new(), selected_visible: 0, cached_entries: Vec::new() }
    }

    pub fn from_files(files: &[FileDiff]) -> Self {
        let mut roots: Vec<FileTreeNode> = Vec::new();

        for (idx, file) in files.iter().enumerate() {
            let parts: Vec<&str> = file.path.split('/').collect();
            insert_into_tree(&mut roots, &parts, idx, file);
        }

        sort_tree(&mut roots);
        compress_paths(&mut roots);

        let mut tree = Self { roots, selected_visible: 0, cached_entries: Vec::new() };
        tree.rebuild_visible_entries();
        tree
    }

    pub fn select_file_index(&mut self, file_index: usize) {
        if let Some(pos) = self
            .cached_entries
            .iter()
            .position(|e| matches!(&e.kind, FileTreeEntryKind::File { file_index: fi, .. } if *fi == file_index))
        {
            self.selected_visible = pos;
        }
    }

    pub fn visible_entries(&self) -> &[FileTreeEntry] {
        &self.cached_entries
    }

    pub fn selected_visible(&self) -> usize {
        self.selected_visible
    }

    pub fn selected_file_index(&self) -> Option<usize> {
        self.cached_entries.get(self.selected_visible).and_then(|e| match &e.kind {
            FileTreeEntryKind::File { file_index, .. } => Some(*file_index),
            FileTreeEntryKind::Directory { .. } => None,
        })
    }

    pub fn navigate(&mut self, delta: isize) {
        if self.cached_entries.is_empty() {
            return;
        }
        crate::components::wrap_selection(&mut self.selected_visible, self.cached_entries.len(), delta);
    }

    pub fn collapse_or_parent(&mut self) {
        let Some(entry) = self.cached_entries.get(self.selected_visible) else {
            return;
        };

        match &entry.kind {
            FileTreeEntryKind::Directory { expanded: true, .. } => {
                toggle_at(&mut self.roots, self.selected_visible);
                self.rebuild_visible_entries();
            }
            _ => {
                if let Some(parent_idx) = find_parent_dir(&self.cached_entries, self.selected_visible) {
                    self.selected_visible = parent_idx;
                }
            }
        }
    }

    pub fn expand_or_enter(&mut self) -> bool {
        let Some(entry) = self.cached_entries.get(self.selected_visible) else {
            return false;
        };

        match &entry.kind {
            FileTreeEntryKind::Directory { expanded: false, .. } => {
                toggle_at(&mut self.roots, self.selected_visible);
                self.rebuild_visible_entries();
                false
            }
            FileTreeEntryKind::Directory { expanded: true, .. } => {
                if self.selected_visible + 1 < self.cached_entries.len() {
                    self.selected_visible += 1;
                }
                false
            }
            FileTreeEntryKind::File { .. } => true,
        }
    }

    fn rebuild_visible_entries(&mut self) {
        let mut entries = Vec::new();
        for node in &self.roots {
            collect_visible(node, 0, &mut entries);
        }
        self.cached_entries = entries;
        if self.cached_entries.is_empty() {
            self.selected_visible = 0;
        } else {
            self.selected_visible = self.selected_visible.min(self.cached_entries.len() - 1);
        }
    }
}

fn insert_into_tree(nodes: &mut Vec<FileTreeNode>, parts: &[&str], file_index: usize, file: &FileDiff) {
    if parts.len() == 1 {
        nodes.push(FileTreeNode::File {
            file_index,
            name: parts[0].to_string(),
            status: file.status,
            additions: file.additions(),
            deletions: file.deletions(),
        });
        return;
    }

    let dir_name = parts[0];
    let existing = nodes.iter_mut().find(|n| matches!(n, FileTreeNode::Directory { name, .. } if name == dir_name));

    if let Some(FileTreeNode::Directory { children, .. }) = existing {
        insert_into_tree(children, &parts[1..], file_index, file);
    } else {
        let mut children = Vec::new();
        insert_into_tree(&mut children, &parts[1..], file_index, file);
        nodes.push(FileTreeNode::Directory { name: dir_name.to_string(), children, expanded: true });
    }
}

fn sort_tree(nodes: &mut [FileTreeNode]) {
    nodes.sort_by(|a, b| {
        let a_is_dir = matches!(a, FileTreeNode::Directory { .. });
        let b_is_dir = matches!(b, FileTreeNode::Directory { .. });
        b_is_dir.cmp(&a_is_dir).then_with(|| node_name(a).cmp(node_name(b)))
    });
    for node in nodes.iter_mut() {
        if let FileTreeNode::Directory { children, .. } = node {
            sort_tree(children);
        }
    }
}

fn compress_paths(nodes: &mut [FileTreeNode]) {
    for node in nodes.iter_mut() {
        loop {
            let should_compress = matches!(
                node,
                FileTreeNode::Directory { children, .. }
                if children.len() == 1 && matches!(children[0], FileTreeNode::Directory { .. })
            );
            if !should_compress {
                break;
            }
            if let FileTreeNode::Directory { name, children, .. } = node {
                let child = children.remove(0);
                if let FileTreeNode::Directory {
                    name: child_name,
                    children: child_children,
                    expanded: child_expanded,
                } = child
                {
                    *name = format!("{name}/{child_name}");
                    *children = child_children;
                    if let FileTreeNode::Directory { expanded, .. } = node {
                        *expanded = child_expanded;
                    }
                }
            }
        }
        if let FileTreeNode::Directory { children, .. } = node {
            compress_paths(children);
        }
    }
}

fn node_name(node: &FileTreeNode) -> &str {
    match node {
        FileTreeNode::Directory { name, .. } | FileTreeNode::File { name, .. } => name,
    }
}

fn collect_visible(node: &FileTreeNode, depth: usize, entries: &mut Vec<FileTreeEntry>) {
    match node {
        FileTreeNode::Directory { name, children, expanded } => {
            entries.push(FileTreeEntry {
                depth,
                kind: FileTreeEntryKind::Directory { name: name.clone(), expanded: *expanded },
            });
            if *expanded {
                for child in children {
                    collect_visible(child, depth + 1, entries);
                }
            }
        }
        FileTreeNode::File { file_index, name, status, additions, deletions } => {
            entries.push(FileTreeEntry {
                depth,
                kind: FileTreeEntryKind::File {
                    file_index: *file_index,
                    name: name.clone(),
                    status: *status,
                    additions: *additions,
                    deletions: *deletions,
                },
            });
        }
    }
}

fn toggle_at(nodes: &mut [FileTreeNode], target_visible_idx: usize) {
    let mut counter = 0;
    toggle_at_inner(nodes, target_visible_idx, &mut counter);
}

fn toggle_at_inner(nodes: &mut [FileTreeNode], target: usize, counter: &mut usize) -> bool {
    for node in nodes.iter_mut() {
        if *counter == target {
            if let FileTreeNode::Directory { expanded, .. } = node {
                *expanded = !*expanded;
            }
            return true;
        }
        *counter += 1;
        if let FileTreeNode::Directory { children, expanded: true, .. } = node
            && toggle_at_inner(children, target, counter)
        {
            return true;
        }
    }
    false
}

fn find_parent_dir(entries: &[FileTreeEntry], idx: usize) -> Option<usize> {
    let current_depth = entries.get(idx)?.depth;
    if current_depth == 0 {
        return None;
    }
    (0..idx)
        .rev()
        .find(|&i| entries[i].depth < current_depth && matches!(entries[i].kind, FileTreeEntryKind::Directory { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, PatchLineKind};

    fn file(path: &str, status: FileStatus, additions: usize, deletions: usize) -> FileDiff {
        let mut lines = Vec::new();
        for i in 0..additions {
            lines.push(PatchLine {
                kind: PatchLineKind::Added,
                text: format!("added {i}"),
                old_line_no: None,
                new_line_no: Some(i + 1),
            });
        }
        for i in 0..deletions {
            lines.push(PatchLine {
                kind: PatchLineKind::Removed,
                text: format!("removed {i}"),
                old_line_no: Some(i + 1),
                new_line_no: None,
            });
        }
        FileDiff {
            old_path: None,
            path: path.to_string(),
            status,
            hunks: if lines.is_empty() {
                vec![]
            } else {
                vec![Hunk {
                    header: "@@ -1 +1 @@".to_string(),
                    old_start: 1,
                    old_count: deletions,
                    new_start: 1,
                    new_count: additions,
                    lines,
                }]
            },
            binary: false,
        }
    }

    fn modified(path: &str) -> FileDiff {
        file(path, FileStatus::Modified, 1, 1)
    }

    fn added(path: &str) -> FileDiff {
        file(path, FileStatus::Added, 2, 0)
    }

    #[test]
    fn from_files_groups_by_directory() {
        let files = vec![modified("src/a.rs"), modified("src/b.rs"), modified("lib/c.rs")];
        let tree = FileTree::from_files(&files);
        let entries = tree.visible_entries();
        assert_eq!(entries.len(), 5);
        assert!(matches!(&entries[0].kind, FileTreeEntryKind::Directory { name, .. } if name == "lib"));
        assert!(matches!(&entries[1].kind, FileTreeEntryKind::File { name, .. } if name == "c.rs"));
        assert!(matches!(&entries[2].kind, FileTreeEntryKind::Directory { name, .. } if name == "src"));
        assert!(matches!(&entries[3].kind, FileTreeEntryKind::File { name, .. } if name == "a.rs"));
        assert!(matches!(&entries[4].kind, FileTreeEntryKind::File { name, .. } if name == "b.rs"));
    }

    #[test]
    fn visible_entries_respects_collapse() {
        let files = vec![modified("src/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        assert_eq!(tree.visible_entries().len(), 3);

        tree.collapse_or_parent();
        assert_eq!(tree.visible_entries().len(), 1);
    }

    #[test]
    fn navigate_wraps() {
        let files = vec![modified("a.rs"), modified("b.rs")];
        let mut tree = FileTree::from_files(&files);
        assert_eq!(tree.selected_visible, 0);
        tree.navigate(1);
        assert_eq!(tree.selected_visible, 1);
        tree.navigate(1);
        assert_eq!(tree.selected_visible, 0);
    }

    #[test]
    fn collapse_or_parent_collapses_dir() {
        let files = vec![modified("src/a.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.selected_visible = 0;
        assert_eq!(tree.visible_entries().len(), 2);
        tree.collapse_or_parent();
        assert_eq!(tree.visible_entries().len(), 1);
    }

    #[test]
    fn collapse_or_parent_moves_to_parent_from_file() {
        let files = vec![modified("src/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.selected_visible = 1;
        tree.collapse_or_parent();
        assert_eq!(tree.selected_visible, 0);
    }

    #[test]
    fn expand_or_enter_returns_true_for_file() {
        let files = vec![modified("a.rs")];
        let mut tree = FileTree::from_files(&files);
        assert!(tree.expand_or_enter());
    }

    #[test]
    fn expand_or_enter_expands_collapsed_dir() {
        let files = vec![modified("src/a.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.collapse_or_parent();
        assert_eq!(tree.visible_entries().len(), 1);
        let result = tree.expand_or_enter();
        assert!(!result);
        assert_eq!(tree.visible_entries().len(), 2);
    }

    #[test]
    fn path_compression_for_single_child_dirs() {
        let files = vec![modified("src/deep/nested/file.rs")];
        let tree = FileTree::from_files(&files);
        let entries = tree.visible_entries();
        assert_eq!(entries.len(), 2);
        match &entries[0].kind {
            FileTreeEntryKind::Directory { name, .. } => {
                assert_eq!(name, "src/deep/nested");
            }
            FileTreeEntryKind::File { .. } => panic!("expected directory"),
        }
    }

    #[test]
    fn selected_file_index_returns_none_for_dir() {
        let files = vec![modified("src/a.rs")];
        let tree = FileTree::from_files(&files);
        assert!(tree.selected_file_index().is_none());
    }

    #[test]
    fn selected_file_index_returns_index_for_file() {
        let files = vec![modified("a.rs")];
        let tree = FileTree::from_files(&files);
        assert_eq!(tree.selected_file_index(), Some(0));
    }

    #[test]
    fn flat_files_no_grouping() {
        let files = vec![modified("a.rs"), added("b.rs")];
        let tree = FileTree::from_files(&files);
        let entries = tree.visible_entries();
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0].kind, FileTreeEntryKind::File { name, .. } if name == "a.rs"));
        assert!(matches!(&entries[1].kind, FileTreeEntryKind::File { name, .. } if name == "b.rs"));
    }

    #[test]
    fn selected_visible_clamped_after_collapse() {
        let files = vec![modified("src/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.selected_visible = 2;
        tree.collapse_or_parent();
        assert!(tree.selected_visible < tree.visible_entries().len());
    }

    #[test]
    fn expand_already_expanded_dir_moves_to_first_child() {
        let files = vec![modified("src/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.selected_visible = 0;
        let result = tree.expand_or_enter();
        assert!(!result);
        assert_eq!(tree.selected_visible, 1);
    }
}
