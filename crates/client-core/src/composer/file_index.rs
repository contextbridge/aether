use crate::composer::attachment::PromptAttachment;
use std::path::Path;

pub fn index_files(root: &Path) -> Vec<PromptAttachment> {
    let mut entries: Vec<_> = ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .hidden(false)
        .parents(true)
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|path| !excluded(path))
        .take(MAX_INDEXED_FILES)
        .map(|path| PromptAttachment::new(path, root))
        .collect();
    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    entries
}

const MAX_INDEXED_FILES: usize = 50_000;

fn excluded(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}
