use crate::git_diff::{FileDiff, FileStatus, StageState};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    pub depth: usize,
    pub kind: FileTreeEntryKind,
}

#[derive(Debug, Clone)]
pub enum FileTreeEntryKind {
    Directory {
        path: String,
        name: String,
        expanded: bool,
        staged: StageState,
        file_indices: Vec<usize>,
    },
    File {
        path: String,
        file_index: usize,
        name: String,
        status: FileStatus,
        staged: StageState,
        additions: usize,
        deletions: usize,
    },
}

pub struct FileTree {
    entries: Vec<FileTreeEntry>,
    visible: Vec<usize>,
    selected_visible: usize,
}

impl FileTree {
    pub fn empty() -> Self {
        Self { entries: Vec::new(), visible: Vec::new(), selected_visible: 0 }
    }

    pub fn from_files(files: &[FileDiff]) -> Self {
        let mut tree = Self::empty();
        tree.rebuild_from_files(files);
        tree
    }

    pub fn rebuild_from_files(&mut self, files: &[FileDiff]) {
        let selected_path = self.selected_entry().map(|entry| entry_path(entry).to_owned());
        let collapsed: HashSet<String> = self
            .entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                FileTreeEntryKind::Directory { path, expanded: false, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();

        self.entries = build_entries(files);
        for entry in &mut self.entries {
            if let FileTreeEntryKind::Directory { path, expanded, .. } = &mut entry.kind {
                *expanded = !collapsed.contains(path);
            }
        }
        self.rebuild_visible();
        if let Some(path) = selected_path
            && let Some(position) = self.visible.iter().position(|&index| entry_path(&self.entries[index]) == path)
        {
            self.selected_visible = position;
        }
    }

    pub fn visible_entries(&self) -> Vec<&FileTreeEntry> {
        self.visible.iter().map(|&index| &self.entries[index]).collect()
    }

    pub fn selected_visible(&self) -> usize {
        self.selected_visible
    }

    pub fn selected_file_index(&self) -> Option<usize> {
        self.selected_entry().and_then(|entry| match &entry.kind {
            FileTreeEntryKind::File { file_index, .. } => Some(*file_index),
            FileTreeEntryKind::Directory { .. } => None,
        })
    }

    pub fn selected_file_indices(&self) -> Vec<usize> {
        match self.selected_entry().map(|entry| &entry.kind) {
            Some(FileTreeEntryKind::File { file_index, .. }) => vec![*file_index],
            Some(FileTreeEntryKind::Directory { file_indices, .. }) => file_indices.clone(),
            None => Vec::new(),
        }
    }

    pub fn select_file_index(&mut self, file_index: usize) {
        if let Some(position) = self.visible.iter().position(|&index| {
            matches!(&self.entries[index].kind, FileTreeEntryKind::File { file_index: fi, .. } if *fi == file_index)
        }) {
            self.selected_visible = position;
        }
    }

    pub fn navigate(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        self.selected_visible = self.selected_visible.saturating_add_signed(delta).min(self.visible.len() - 1);
    }

    pub fn collapse_or_parent(&mut self) {
        let Some(&index) = self.visible.get(self.selected_visible) else {
            return;
        };
        if let FileTreeEntryKind::Directory { expanded, .. } = &mut self.entries[index].kind
            && *expanded
        {
            *expanded = false;
            self.rebuild_visible();
            return;
        }
        if let Some(parent) = self.parent_position(self.selected_visible) {
            self.selected_visible = parent;
        }
    }

    pub fn expand_or_enter(&mut self) -> bool {
        let Some(&index) = self.visible.get(self.selected_visible) else {
            return false;
        };
        match &mut self.entries[index].kind {
            FileTreeEntryKind::File { .. } => true,
            FileTreeEntryKind::Directory { expanded, .. } => {
                if *expanded {
                    if self.selected_visible + 1 < self.visible.len() {
                        self.selected_visible += 1;
                    }
                } else {
                    *expanded = true;
                    self.rebuild_visible();
                }
                false
            }
        }
    }

    fn selected_entry(&self) -> Option<&FileTreeEntry> {
        self.visible.get(self.selected_visible).map(|&index| &self.entries[index])
    }

    fn parent_position(&self, position: usize) -> Option<usize> {
        let current_depth = self.entries[*self.visible.get(position)?].depth;
        if current_depth == 0 {
            return None;
        }
        (0..position).rev().find(|&candidate| {
            let entry = &self.entries[self.visible[candidate]];
            entry.depth < current_depth && matches!(entry.kind, FileTreeEntryKind::Directory { .. })
        })
    }

    fn rebuild_visible(&mut self) {
        self.visible.clear();
        let mut hidden_below: Option<usize> = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if entry.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            self.visible.push(index);
            if matches!(entry.kind, FileTreeEntryKind::Directory { expanded: false, .. }) {
                hidden_below = Some(entry.depth);
            }
        }
        self.selected_visible = self.selected_visible.min(self.visible.len().saturating_sub(1));
    }
}

enum BuildNode {
    Directory { name: String, children: Vec<BuildNode> },
    File { file_index: usize, name: String },
}

fn build_entries(files: &[FileDiff]) -> Vec<FileTreeEntry> {
    let mut roots: Vec<BuildNode> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let parts: Vec<&str> = file.path.split('/').collect();
        insert_into_tree(&mut roots, &parts, index);
    }
    sort_tree(&mut roots);
    compress_paths(&mut roots);

    let mut entries = Vec::new();
    flatten_into(&roots, 0, "", files, &mut entries);
    entries
}

fn insert_into_tree(nodes: &mut Vec<BuildNode>, parts: &[&str], file_index: usize) {
    if parts.len() == 1 {
        nodes.push(BuildNode::File { file_index, name: parts[0].to_string() });
        return;
    }

    let dir_name = parts[0];
    let existing = nodes.iter_mut().find(|node| matches!(node, BuildNode::Directory { name, .. } if name == dir_name));

    if let Some(BuildNode::Directory { children, .. }) = existing {
        insert_into_tree(children, &parts[1..], file_index);
    } else {
        let mut children = Vec::new();
        insert_into_tree(&mut children, &parts[1..], file_index);
        nodes.push(BuildNode::Directory { name: dir_name.to_string(), children });
    }
}

fn sort_tree(nodes: &mut [BuildNode]) {
    nodes.sort_by(|a, b| {
        let a_is_dir = matches!(a, BuildNode::Directory { .. });
        let b_is_dir = matches!(b, BuildNode::Directory { .. });
        b_is_dir.cmp(&a_is_dir).then_with(|| node_name(a).cmp(node_name(b)))
    });
    for node in nodes.iter_mut() {
        if let BuildNode::Directory { children, .. } = node {
            sort_tree(children);
        }
    }
}

fn compress_paths(nodes: &mut [BuildNode]) {
    for node in nodes.iter_mut() {
        while let BuildNode::Directory { name, children } = node {
            if children.len() != 1 || !matches!(children[0], BuildNode::Directory { .. }) {
                break;
            }
            let BuildNode::Directory { name: child_name, children: grandchildren } = children.remove(0) else {
                unreachable!("checked above that the only child is a directory");
            };
            *name = format!("{name}/{child_name}");
            *children = grandchildren;
        }
        if let BuildNode::Directory { children, .. } = node {
            compress_paths(children);
        }
    }
}

fn node_name(node: &BuildNode) -> &str {
    match node {
        BuildNode::Directory { name, .. } | BuildNode::File { name, .. } => name,
    }
}

fn flatten_into(
    nodes: &[BuildNode],
    depth: usize,
    parent_path: &str,
    files: &[FileDiff],
    entries: &mut Vec<FileTreeEntry>,
) -> Vec<usize> {
    let mut collected = Vec::new();
    for node in nodes {
        match node {
            BuildNode::Directory { name, children } => {
                let path = join_path(parent_path, name);
                let slot = entries.len();
                entries.push(FileTreeEntry {
                    depth,
                    kind: FileTreeEntryKind::Directory {
                        path: path.clone(),
                        name: name.clone(),
                        expanded: true,
                        staged: StageState::Unstaged,
                        file_indices: Vec::new(),
                    },
                });
                let descendants = flatten_into(children, depth + 1, &path, files, entries);
                if let FileTreeEntryKind::Directory { staged, file_indices, .. } = &mut entries[slot].kind {
                    *staged = aggregate_stage_state(&descendants, files);
                    file_indices.clone_from(&descendants);
                }
                collected.extend(descendants);
            }
            BuildNode::File { file_index, name } => {
                let file = &files[*file_index];
                entries.push(FileTreeEntry {
                    depth,
                    kind: FileTreeEntryKind::File {
                        path: join_path(parent_path, name),
                        file_index: *file_index,
                        name: name.clone(),
                        status: file.status,
                        staged: file.staged,
                        additions: file.additions(),
                        deletions: file.deletions(),
                    },
                });
                collected.push(*file_index);
            }
        }
    }
    collected
}

fn aggregate_stage_state(file_indices: &[usize], files: &[FileDiff]) -> StageState {
    let mut states = file_indices.iter().map(|&index| files[index].staged);
    let Some(first) = states.next() else {
        return StageState::Unstaged;
    };
    if states.all(|state| state == first) { first } else { StageState::PartiallyStaged }
}

fn entry_path(entry: &FileTreeEntry) -> &str {
    match &entry.kind {
        FileTreeEntryKind::Directory { path, .. } | FileTreeEntryKind::File { path, .. } => path,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, StageState};

    fn file(path: &str, status: FileStatus, additions: usize, deletions: usize) -> FileDiff {
        let mut lines = Vec::new();
        for i in 0..additions {
            lines.push(PatchLine::added(format!("added {i}"), i + 1));
        }
        for i in 0..deletions {
            lines.push(PatchLine::removed(format!("removed {i}"), i + 1));
        }
        FileDiff {
            old_path: None,
            path: path.to_string(),
            status,
            staged: StageState::Unstaged,
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
    fn navigate_clamps_at_bounds() {
        let files = vec![modified("a.rs"), modified("b.rs")];
        let mut tree = FileTree::from_files(&files);
        assert_eq!(tree.selected_visible, 0);
        tree.navigate(1);
        assert_eq!(tree.selected_visible, 1);
        tree.navigate(1);
        assert_eq!(tree.selected_visible, 1, "navigating past the last entry should stay on it");
        tree.navigate(-1);
        assert_eq!(tree.selected_visible, 0);
        tree.navigate(-1);
        assert_eq!(tree.selected_visible, 0, "navigating before the first entry should stay on it");
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
    fn rebuild_preserves_selected_directory() {
        let files = vec![modified("lib/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.navigate(2);

        tree.rebuild_from_files(&files);

        assert!(
            matches!(&tree.visible_entries()[tree.selected_visible()].kind, FileTreeEntryKind::Directory { name, .. } if name == "src")
        );
    }

    #[test]
    fn rebuild_preserves_collapsed_directories() {
        let files = vec![modified("lib/a.rs"), modified("src/b.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.collapse_or_parent();

        tree.rebuild_from_files(&files);

        assert!(
            matches!(&tree.visible_entries()[0].kind, FileTreeEntryKind::Directory { name, expanded: false, .. } if name == "lib")
        );
        assert!(
            !tree
                .visible_entries()
                .iter()
                .any(|entry| matches!(&entry.kind, FileTreeEntryKind::File { name, .. } if name == "a.rs"))
        );
    }

    #[test]
    fn rebuild_preserves_collapsed_directory_hidden_under_collapsed_parent() {
        let files = vec![modified("src/inner/a.rs"), modified("src/b.rs"), modified("src/inner/other/c.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.navigate(1);
        tree.collapse_or_parent();
        tree.collapse_or_parent();
        tree.collapse_or_parent();
        assert_eq!(tree.visible_entries().len(), 1);

        tree.rebuild_from_files(&files);
        tree.expand_or_enter();

        assert!(
            matches!(&tree.visible_entries()[1].kind, FileTreeEntryKind::Directory { name, expanded: false, .. } if name == "inner"),
            "nested collapsed directory should stay collapsed after rebuild"
        );
    }

    #[test]
    fn directory_stage_state_aggregates_descendant_files() {
        let mut files = vec![modified("src/a.rs"), modified("src/b.rs")];
        files[1].staged = StageState::Staged;
        let tree = FileTree::from_files(&files);

        assert!(matches!(
            &tree.visible_entries()[0].kind,
            FileTreeEntryKind::Directory { staged: StageState::PartiallyStaged, .. }
        ));
    }

    #[test]
    fn selected_file_indices_for_directory_includes_hidden_descendants() {
        let files = vec![modified("src/nested/a.rs"), modified("src/b.rs"), modified("top.rs")];
        let mut tree = FileTree::from_files(&files);
        tree.collapse_or_parent();

        let mut indices = tree.selected_file_indices();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1]);
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
