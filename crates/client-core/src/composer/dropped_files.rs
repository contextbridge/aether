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
    use std::fs;
    use utils::testing::TestWorkspace;

    fn setup_files(workspace: &TestWorkspace, names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|name| workspace.write(name, b"test")).collect()
    }

    #[test]
    fn file_uri_for_png_parses_to_local_path() {
        let tmp = TestWorkspace::new();
        let paths = setup_files(&tmp, &["image.png"]);
        let uri = format!("file://{}", paths[0].display());

        let result = parse_dropped_file_paths(&uri).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn absolute_posix_path_parses_correctly() {
        let tmp = TestWorkspace::new();
        let paths = setup_files(&tmp, &["photo.jpg"]);
        let input = paths[0].to_str().unwrap();

        let result = parse_dropped_file_paths(input).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn quoted_path_with_spaces_parses_correctly() {
        let tmp = TestWorkspace::new();
        let p = tmp.path("My File.png");
        fs::write(&p, b"test").unwrap();

        let single_quoted = format!("'{}'", p.display());
        let result = parse_dropped_file_paths(&single_quoted).unwrap();
        assert_eq!(result, vec![p.clone()]);

        let double_quoted = format!("\"{}\"", p.display());
        let result = parse_dropped_file_paths(&double_quoted).unwrap();
        assert_eq!(result, vec![p]);
    }

    #[test]
    fn shell_escaped_path_with_spaces_parses_correctly() {
        let tmp = TestWorkspace::new();
        let p = tmp.path("My File.png");
        fs::write(&p, b"test").unwrap();

        let escaped = p.to_str().unwrap().replace(' ', "\\ ");
        let result = parse_dropped_file_paths(&escaped).unwrap();
        assert_eq!(result, vec![p]);
    }

    #[test]
    fn multiple_dropped_files_parse_correctly() {
        let tmp = TestWorkspace::new();
        let paths = setup_files(&tmp, &["a.png", "b.wav"]);
        let input = format!("{}\n{}", paths[0].display(), paths[1].display());

        let result = parse_dropped_file_paths(&input).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn multiple_file_uris_parse_correctly() {
        let tmp = TestWorkspace::new();
        let paths = setup_files(&tmp, &["a.png", "b.wav"]);
        let input = format!("file://{}\nfile://{}", paths[0].display(), paths[1].display());

        let result = parse_dropped_file_paths(&input).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn ordinary_text_returns_none() {
        assert!(parse_dropped_file_paths("hello world").is_none());
        assert!(parse_dropped_file_paths("some random text\nwith newlines").is_none());
        assert!(parse_dropped_file_paths("let x = 42;").is_none());
    }

    #[test]
    fn missing_absolute_path_is_parsed_without_filesystem_io() {
        assert_eq!(
            parse_dropped_file_paths("/nonexistent/path/to/file.png"),
            Some(vec![PathBuf::from("/nonexistent/path/to/file.png")])
        );
    }

    #[test]
    fn directory_shaped_absolute_path_is_parsed_lexically() {
        let tmp = TestWorkspace::new();
        let input = tmp.root().to_str().unwrap();
        assert_eq!(parse_dropped_file_paths(input), Some(vec![tmp.root().to_path_buf()]));
    }

    #[test]
    fn empty_string_returns_none() {
        assert!(parse_dropped_file_paths("").is_none());
        assert!(parse_dropped_file_paths("   ").is_none());
    }

    #[test]
    fn mixed_valid_and_invalid_returns_none() {
        let tmp = TestWorkspace::new();
        let paths = setup_files(&tmp, &["a.png"]);
        let input = format!("{}\nhello world", paths[0].display());
        assert!(parse_dropped_file_paths(&input).is_none());
    }
}
