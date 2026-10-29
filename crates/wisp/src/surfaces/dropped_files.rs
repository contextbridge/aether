use std::path::PathBuf;
use url::Url;

/// Attempts to parse a paste payload as one or more dropped file paths.
///
/// Returns `Some(paths)` when the payload is high-confidence file-drop content,
/// `None` when it looks like ordinary user text and should remain a normal paste.
pub(crate) fn parse_dropped_file_paths(pasted: &str) -> Option<Vec<PathBuf>> {
    pasted
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(parse_single_path)
        .collect::<Option<Vec<_>>>()
        .filter(|paths| !paths.is_empty())
}

fn parse_single_path(input: &str) -> Option<PathBuf> {
    if input.starts_with("file://") {
        return Url::parse(input).ok()?.to_file_path().ok();
    }
    match ['\'', '"'].iter().find_map(|&quote| input.strip_prefix(quote)?.strip_suffix(quote)) {
        Some(inner) => inner.starts_with('/').then(|| PathBuf::from(inner)),
        None => input.starts_with('/').then(|| PathBuf::from(input.replace("\\ ", " "))),
    }
}
