use agent_client_protocol::schema::v2::{AbsolutePath, Diff, DiffChange, DiffFileType, DiffPatch};
use clankerdiff_core::git_patch_from_texts;
use mcp_utils::display_meta::FileDiff;

/// Convert full text snapshots into Git patch
pub fn map_file_diff(diff: &FileDiff) -> Option<Diff> {
    if !std::path::Path::new(&diff.path).is_absolute() {
        return None;
    }
    let patch = git_patch_from_texts(&diff.path, diff.old_text.as_deref(), diff.new_text.as_deref()).ok()?;
    let file_path = AbsolutePath::new(diff.path.clone());
    let change = match (&diff.old_text, &diff.new_text) {
        (None, Some(_)) => DiffChange::add(file_path),
        (Some(_), None) => DiffChange::delete(file_path),
        (Some(_), Some(_)) => DiffChange::modify(file_path),
        (None, None) => return None,
    }
    .file_type(DiffFileType::Text)
    .mime_type("text/plain");
    Some(Diff::new(vec![change]).with_patch(patch.map(DiffPatch::new)))
}
