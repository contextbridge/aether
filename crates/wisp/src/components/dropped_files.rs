use std::path::PathBuf;

/// Attempts to parse a paste payload as one or more dropped file paths.
///
/// Returns `Some(paths)` when the payload is high-confidence file-drop content,
/// `None` when it looks like ordinary user text and should remain a normal paste.
pub fn parse_dropped_file_paths(pasted: &str) -> Option<Vec<PathBuf>> {
    let trimmed = pasted.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidates: Vec<&str> = trimmed.lines().collect();
    let mut paths = Vec::new();

    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        if let Some(path) = try_parse_single_path(candidate) {
            paths.push(path);
        } else {
            // If any line doesn't look like a path, treat the whole payload as plain text
            return None;
        }
    }

    if paths.is_empty() {
        return None;
    }

    // Validate all paths exist and are files
    let valid: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();

    if valid.is_empty() {
        return None;
    }

    Some(valid)
}

fn try_parse_single_path(input: &str) -> Option<PathBuf> {
    // file:// URI
    if let Some(path) = try_parse_file_uri(input) {
        return Some(path);
    }

    // Quoted path: 'path' or "path"
    if let Some(path) = try_parse_quoted_path(input) {
        return Some(path);
    }

    // Shell-escaped path (contains backslash-space)
    if input.contains("\\ ") && looks_like_absolute_path(input.split("\\ ").next().unwrap_or("")) {
        let unescaped = input.replace("\\ ", " ");
        return Some(PathBuf::from(unescaped));
    }

    // Plain absolute path
    if looks_like_absolute_path(input) {
        return Some(PathBuf::from(input));
    }

    None
}

fn try_parse_file_uri(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    if !input.starts_with("file://") {
        return None;
    }
    url::Url::parse(input).ok().and_then(|u| u.to_file_path().ok())
}

fn try_parse_quoted_path(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    let inner =
        if (input.starts_with('\'') && input.ends_with('\'')) || (input.starts_with('"') && input.ends_with('"')) {
            &input[1..input.len() - 1]
        } else {
            return None;
        };

    if looks_like_absolute_path(inner) { Some(PathBuf::from(inner)) } else { None }
}

fn looks_like_absolute_path(s: &str) -> bool {
    s.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_files(dir: &TempDir, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                fs::write(&p, b"test").unwrap();
                p
            })
            .collect()
    }

    #[test]
    fn file_uri_for_png_parses_to_local_path() {
        let tmp = TempDir::new().unwrap();
        let paths = setup_files(&tmp, &["image.png"]);
        let uri = format!("file://{}", paths[0].display());

        let result = parse_dropped_file_paths(&uri).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn absolute_posix_path_parses_correctly() {
        let tmp = TempDir::new().unwrap();
        let paths = setup_files(&tmp, &["photo.jpg"]);
        let input = paths[0].to_str().unwrap();

        let result = parse_dropped_file_paths(input).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn quoted_path_with_spaces_parses_correctly() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("My File.png");
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
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("My File.png");
        fs::write(&p, b"test").unwrap();

        let escaped = p.to_str().unwrap().replace(' ', "\\ ");
        let result = parse_dropped_file_paths(&escaped).unwrap();
        assert_eq!(result, vec![p]);
    }

    #[test]
    fn multiple_dropped_files_parse_correctly() {
        let tmp = TempDir::new().unwrap();
        let paths = setup_files(&tmp, &["a.png", "b.wav"]);
        let input = format!("{}\n{}", paths[0].display(), paths[1].display());

        let result = parse_dropped_file_paths(&input).unwrap();
        assert_eq!(result, paths);
    }

    #[test]
    fn multiple_file_uris_parse_correctly() {
        let tmp = TempDir::new().unwrap();
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
    fn missing_path_is_rejected() {
        assert!(parse_dropped_file_paths("/nonexistent/path/to/file.png").is_none());
    }

    #[test]
    fn directory_path_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let input = tmp.path().to_str().unwrap();
        assert!(parse_dropped_file_paths(input).is_none());
    }

    #[test]
    fn empty_string_returns_none() {
        assert!(parse_dropped_file_paths("").is_none());
        assert!(parse_dropped_file_paths("   ").is_none());
    }

    #[test]
    fn mixed_valid_and_invalid_returns_none() {
        let tmp = TempDir::new().unwrap();
        let paths = setup_files(&tmp, &["a.png"]);
        let input = format!("{}\nhello world", paths[0].display());
        assert!(parse_dropped_file_paths(&input).is_none());
    }
}
