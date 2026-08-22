use std::path::{Path, PathBuf};

pub(crate) const MAX_INDEXED_FILES: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub display_name: String,
}

/// Walks the working tree for the files the `@` picker can offer.
pub fn index_files(root: &Path) -> Vec<FileEntry> {
    index_files_up_to(root, MAX_INDEXED_FILES)
}

#[cfg(feature = "testing")]
pub fn index_files_with_limit(root: &Path, limit: usize) -> Vec<FileEntry> {
    index_files_up_to(root, limit)
}

pub(crate) fn file_entries(
    root: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
    limit: usize,
) -> Vec<FileEntry> {
    let mut entries: Vec<_> = paths
        .into_iter()
        .filter(|path| !excluded(path))
        .take(limit)
        .map(|path| FileEntry {
            display_name: path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/"),
            path,
        })
        .collect();
    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    entries
}

fn index_files_up_to(root: &Path, limit: usize) -> Vec<FileEntry> {
    let paths = ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .hidden(false)
        .parents(true)
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path);
    file_entries(root, paths, limit)
}

fn excluded(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}
