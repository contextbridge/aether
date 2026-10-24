use std::path::PathBuf;
use url::Url;

/// Attempts to parse a paste payload as one or more dropped file paths.
///
/// Returns `Some(paths)` when the payload is high-confidence file-drop content,
/// `None` when it looks like ordinary user text and should remain a normal paste.
pub fn parse_dropped_file_paths(pasted: &str) -> Option<Vec<PathBuf>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_read_without_touching_the_filesystem() {
        let cases: [(&str, &[&str]); 9] = [
            ("/nonexistent/photo.jpg", &["/nonexistent/photo.jpg"]),
            ("/nonexistent/folder", &["/nonexistent/folder"]),
            ("file:///nonexistent/image.png", &["/nonexistent/image.png"]),
            ("'/nonexistent/My File.png'", &["/nonexistent/My File.png"]),
            ("\"/nonexistent/My File.png\"", &["/nonexistent/My File.png"]),
            ("/nonexistent/My\\ File.png", &["/nonexistent/My File.png"]),
            ("/nonexistent/a.png\n/nonexistent/b.wav", &["/nonexistent/a.png", "/nonexistent/b.wav"]),
            ("file:///nonexistent/a.png\nfile:///nonexistent/b.wav", &["/nonexistent/a.png", "/nonexistent/b.wav"]),
            ("  /nonexistent/a.png  \n\n", &["/nonexistent/a.png"]),
        ];

        for (pasted, expected) in cases {
            let expected = expected.iter().map(PathBuf::from).collect();
            assert_eq!(parse_dropped_file_paths(pasted), Some(expected), "{pasted:?}");
        }
    }

    #[test]
    fn a_paste_that_is_not_only_paths_stays_text() {
        let pastes =
            ["", "   ", "hello world", "some random text\nwith newlines", "let x = 42;", "/nonexistent/a.png\nhello"];

        for pasted in pastes {
            assert_eq!(parse_dropped_file_paths(pasted), None, "{pasted:?}");
        }
    }
}
