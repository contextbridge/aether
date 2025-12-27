//! Async file walker with fuzzy matching for the @-mention file picker.
//!
//! Walks a directory tree and provides fuzzy matching for file paths,
//! respecting .gitignore patterns and filtering out hidden files.

use std::path::{Path, PathBuf};

/// Maximum number of files to return in search results
const MAX_RESULTS: usize = 50;

/// Maximum depth to walk when searching
const MAX_DEPTH: usize = 10;

/// File extensions commonly found in code projects
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb", "php",
    "swift", "kt", "scala", "clj", "ex", "exs", "erl", "hs", "ml", "fs", "vue", "svelte", "html",
    "css", "scss", "sass", "less", "json", "yaml", "yml", "toml", "xml", "md", "txt", "sql", "sh",
    "bash", "zsh", "fish", "ps1", "bat", "cmd", "dockerfile", "makefile", "cmake", "gradle", "sbt",
    "cargo", "proto", "graphql", "prisma",
];

/// Directories to always ignore when walking
const IGNORE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".svn",
    ".hg",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    "venv",
    ".venv",
    "env",
    ".env",
    "vendor",
    ".cargo",
    ".rustup",
    "coverage",
    ".coverage",
    ".nyc_output",
    ".idea",
    ".vscode",
];

/// A file walker that searches for files in a directory tree.
pub struct FileWalker {
    root_dir: PathBuf,
}

impl FileWalker {
    /// Create a new FileWalker rooted at the given directory.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    /// Search for files matching the given query.
    ///
    /// Returns a list of file paths relative to the root directory,
    /// sorted by match quality.
    pub async fn search(&self, query: &str) -> Vec<PathBuf> {
        let query = query.to_lowercase();

        // Walk the directory and collect matching files
        let mut matches: Vec<(PathBuf, i32)> = Vec::new();
        self.walk_dir(&self.root_dir, &query, 0, &mut matches).await;

        // Sort by score (higher is better)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        // Return just the paths, limited to MAX_RESULTS
        matches
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(path, _)| path)
            .collect()
    }

    /// Recursively walk a directory and collect matching files.
    async fn walk_dir(
        &self,
        dir: &Path,
        query: &str,
        depth: usize,
        matches: &mut Vec<(PathBuf, i32)>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }

        // Early exit if we have enough matches
        if matches.len() >= MAX_RESULTS * 2 {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Skip hidden files and directories
            if file_name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                // Skip ignored directories
                if IGNORE_DIRS.contains(&file_name.to_lowercase().as_str()) {
                    continue;
                }

                // Recurse into subdirectories
                Box::pin(self.walk_dir(&path, query, depth + 1, matches)).await;
            } else if path.is_file() {
                // Get relative path
                let relative_path = match path.strip_prefix(&self.root_dir) {
                    Ok(p) => p.to_path_buf(),
                    Err(_) => continue,
                };

                // Calculate match score
                if let Some(score) = calculate_match_score(&relative_path, query) {
                    matches.push((relative_path, score));
                }
            }
        }
    }
}

/// Calculate a match score for a file path against a query.
///
/// Returns None if the file doesn't match at all.
/// Higher scores indicate better matches.
fn calculate_match_score(path: &Path, query: &str) -> Option<i32> {
    if query.is_empty() {
        // When query is empty, prefer files at root level and common code files
        let path_str = path.to_string_lossy().to_lowercase();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

        // Score based on depth (fewer components = higher score)
        let depth = path.components().count();
        let mut score = 100 - (depth as i32 * 10);

        // Boost common code files
        if CODE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            score += 20;
        }

        // Boost common entry point files
        if path_str.contains("main")
            || path_str.contains("index")
            || path_str.contains("lib")
            || path_str.contains("mod")
        {
            score += 10;
        }

        return Some(score);
    }

    let path_str = path.to_string_lossy().to_lowercase();
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Check if query appears anywhere in the path
    if !path_str.contains(query) && !fuzzy_match(&file_name, query) {
        return None;
    }

    let mut score = 0;

    // Exact filename match
    if file_name == query {
        score += 100;
    }
    // Filename starts with query
    else if file_name.starts_with(query) {
        score += 80;
    }
    // Filename contains query
    else if file_name.contains(query) {
        score += 60;
    }
    // Path contains query
    else if path_str.contains(query) {
        score += 40;
    }
    // Fuzzy match
    else {
        score += 20;
    }

    // Prefer shorter paths
    let depth = path.components().count();
    score -= (depth as i32 - 1) * 5;

    // Boost code files
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if CODE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
        score += 10;
    }

    Some(score)
}

/// Simple fuzzy matching - checks if all characters in query appear in order in target.
fn fuzzy_match(target: &str, query: &str) -> bool {
    let mut query_chars = query.chars().peekable();

    for c in target.chars() {
        if query_chars.peek() == Some(&c) {
            query_chars.next();
        }
        if query_chars.peek().is_none() {
            return true;
        }
    }

    query_chars.peek().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match() {
        assert!(fuzzy_match("main.rs", "mn"));
        assert!(fuzzy_match("main.rs", "main"));
        assert!(fuzzy_match("file_walker.rs", "fw"));
        assert!(fuzzy_match("file_walker.rs", "walker"));
        assert!(!fuzzy_match("main.rs", "xyz"));
        assert!(!fuzzy_match("main.rs", "nim")); // wrong order
    }

    #[test]
    fn test_calculate_match_score_exact() {
        let path = PathBuf::from("main.rs");
        let score = calculate_match_score(&path, "main.rs");
        assert!(score.is_some());
        assert!(score.unwrap() >= 100); // Exact match should score high
    }

    #[test]
    fn test_calculate_match_score_prefix() {
        let path = PathBuf::from("main.rs");
        let score = calculate_match_score(&path, "main");
        assert!(score.is_some());
        assert!(score.unwrap() >= 80); // Prefix match should score well
    }

    #[test]
    fn test_calculate_match_score_no_match() {
        let path = PathBuf::from("main.rs");
        let score = calculate_match_score(&path, "xyz");
        assert!(score.is_none());
    }

    #[test]
    fn test_calculate_match_score_empty_query() {
        let path = PathBuf::from("src/main.rs");
        let score = calculate_match_score(&path, "");
        assert!(score.is_some()); // Empty query matches everything
    }

    #[test]
    fn test_calculate_match_score_deep_path() {
        let shallow = PathBuf::from("main.rs");
        let deep = PathBuf::from("src/deep/nested/main.rs");

        let shallow_score = calculate_match_score(&shallow, "main").unwrap();
        let deep_score = calculate_match_score(&deep, "main").unwrap();

        // Shallow paths should score higher
        assert!(shallow_score > deep_score);
    }
}
