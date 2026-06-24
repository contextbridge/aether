use crate::coding::error::FindError;
use crate::coding::tools::glob_filter::PathGlobMatcher;
use ignore::{WalkBuilder, WalkState};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindInput {
    /// Glob pattern for file discovery.
    pub pattern: String,
    /// The directory to search in (defaults to cwd)
    pub path: Option<String>,
    /// Maximum number of matches to return. Limited searches stop as soon as enough matches are found.
    pub limit: Option<usize>,
    /// Include hidden files and directories (defaults to false)
    #[serde(alias = "include_hidden")]
    pub include_hidden: Option<bool>,
    /// Match patterns case-insensitively (defaults to false)
    #[serde(alias = "case_insensitive")]
    pub case_insensitive: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindOutput {
    /// Array of matching file paths
    pub matches: Vec<String>,
    /// Number of matches returned
    pub count: usize,
    /// Whether the search stopped after reaching the limit
    pub truncated: bool,
    /// Search directory used
    pub search_path: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn find_files(args: FindInput) -> Result<FindOutput, FindError> {
    let search_path = args.path.as_deref().unwrap_or(".");
    let search_root = Path::new(search_path);

    if !search_root.exists() {
        return Err(FindError::PathNotFound(search_path.to_string()));
    }

    let path_matcher = Arc::new(PathGlobMatcher::new(&args.pattern, args.case_insensitive.unwrap_or(false))?);
    let state = Arc::new(FindState::new(args.limit));

    let mut walker_builder = WalkBuilder::new(search_root);
    walker_builder.hidden(!args.include_hidden.unwrap_or(false)).git_ignore(true).follow_links(false);

    walker_builder.build_parallel().run(|| {
        let path_matcher = path_matcher.clone();
        let state = state.clone();

        Box::new(move |result| {
            if state.limit_reached() {
                return WalkState::Quit;
            }

            let Ok(entry) = result else {
                return WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                return WalkState::Continue;
            }

            if path_matcher.matches(entry.path(), search_root) {
                state.push(entry.path().to_string_lossy().to_string());
            }

            if state.limit_reached() { WalkState::Quit } else { WalkState::Continue }
        })
    });

    let (matches, truncated) = state.results()?;
    let count = matches.len();

    let display_meta = ToolDisplayMeta::new(
        "Find files",
        if truncated {
            format!("'{}' ({count}+ files)", args.pattern)
        } else {
            format!("'{}' ({count} files)", args.pattern)
        },
    );

    Ok(FindOutput { matches, count, truncated, search_path: search_path.to_string(), meta: Some(display_meta.into()) })
}

/// Thread-safe accumulator for matches discovered during the parallel walk.
struct FindState {
    matches: Mutex<Vec<String>>,
    limit: Option<usize>,
    limit_reached: AtomicBool,
}

impl FindState {
    fn new(limit: Option<usize>) -> Self {
        Self { matches: Mutex::new(Vec::new()), limit, limit_reached: AtomicBool::new(false) }
    }

    fn push(&self, path: String) {
        if self.limit_reached() {
            return;
        }

        let Ok(mut matches) = self.matches.lock() else {
            return;
        };

        let capacity = self.capacity();
        if matches.len() < capacity {
            matches.push(path);
        }
        if matches.len() >= capacity {
            self.limit_reached.store(true, Ordering::Release);
        }
    }

    fn limit_reached(&self) -> bool {
        self.limit_reached.load(Ordering::Acquire)
    }

    /// Sorted matches, capped to `limit`, plus whether the search truncated.
    fn results(&self) -> Result<(Vec<String>, bool), FindError> {
        let mut matches = self.matches.lock().map(|matches| matches.clone()).map_err(|_| FindError::LockFailed)?;
        matches.sort();
        let truncated = self.limit.is_some_and(|limit| matches.len() > limit);
        if let Some(limit) = self.limit {
            matches.truncate(limit);
        }
        Ok((matches, truncated))
    }

    /// One past `limit`, so a single extra match reveals the result was truncated.
    fn capacity(&self) -> usize {
        self.limit.map_or(usize::MAX, |limit| limit.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_exact_pattern_match() {
        let result = FindTest::new().with_file("test.rs").find("**/test.rs").await;

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("test.rs"));
    }

    #[tokio::test]
    async fn test_glob_wildcard_pattern() {
        let result = FindTest::new()
            .with_file("test.rs")
            .with_file("main.rs")
            .with_file("lib.rs")
            .with_file("notes.txt")
            .with_file("subdir/nested.rs")
            .find("**/*.rs")
            .await;

        assert_eq!(result.count, 4);
        assert!(
            result
                .matches
                .iter()
                .all(|p| { std::path::Path::new(p).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("rs")) })
        );
    }

    #[tokio::test]
    async fn test_bare_exact_filename_matches_any_depth() {
        let result = FindTest::new().with_file("justfile").with_file("subdir/justfile").find("justfile").await;

        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("justfile")));
        assert!(result.matches.iter().any(|p| p.ends_with("subdir/justfile")));
    }

    #[tokio::test]
    async fn test_bare_prefix_glob_matches_basename() {
        let result = FindTest::new().with_file("README.md").with_file("docs/README.adoc").find("README*").await;
        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("README.md")));
        assert!(result.matches.iter().any(|p| p.ends_with("docs/README.adoc")));
    }

    #[tokio::test]
    async fn test_bare_glob_matches_nested_files() {
        let result = FindTest::new()
            .with_file("tsconfig.base.json")
            .with_file("packages/foo/tsconfig.json")
            .find("tsconfig*.json")
            .await;
        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("tsconfig.base.json")));
        assert!(result.matches.iter().any(|p| p.ends_with("packages/foo/tsconfig.json")));
    }

    #[tokio::test]
    async fn test_relative_path_pattern_matches_relative_to_search_root() {
        let result =
            FindTest::new().with_file("crates/example/src/lib.rs").with_file("lib.rs").find("crates/**/*.rs").await;
        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("crates/example/src/lib.rs"));
    }

    #[tokio::test]
    async fn test_limit_truncates_results() {
        let test = FindTest::new().with_file("one.rs").with_file("two.rs").with_file("three.rs");
        let result =
            test.find_with(FindInput { pattern: "*.rs".to_string(), limit: Some(2), ..FindInput::default() }).await;

        assert_eq!(result.count, 2);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn test_limit_not_reached_returns_all_matches() {
        let test = FindTest::new().with_file("one.rs").with_file("two.rs");
        let result =
            test.find_with(FindInput { pattern: "*.rs".to_string(), limit: Some(10), ..FindInput::default() }).await;

        assert_eq!(result.count, 2);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_zero_limit_detects_truncation_without_returning_matches() {
        let test = FindTest::new().with_file("one.rs");
        let result =
            test.find_with(FindInput { pattern: "*.rs".to_string(), limit: Some(0), ..FindInput::default() }).await;

        assert_eq!(result.count, 0);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn test_hidden_files_are_skipped_by_default() {
        let result = FindTest::new().with_file(".aether/settings.json").find("settings.json").await;
        assert_eq!(result.count, 0);
    }

    #[tokio::test]
    async fn test_include_hidden_finds_hidden_files() {
        let test = FindTest::new().with_file(".aether/settings.json");
        let result = test
            .find_with(FindInput {
                pattern: "settings.json".to_string(),
                include_hidden: Some(true),
                ..FindInput::default()
            })
            .await;

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with(".aether/settings.json"));
    }

    #[tokio::test]
    async fn test_case_insensitive_matching() {
        let test = FindTest::new().with_file("README.md");
        let result = test
            .find_with(FindInput {
                pattern: "readme*".to_string(),
                case_insensitive: Some(true),
                ..FindInput::default()
            })
            .await;

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("README.md"));
    }

    #[tokio::test]
    async fn test_validation_error_invalid_path() {
        let args = FindInput {
            pattern: "**/*.rs".to_string(),
            path: Some("/nonexistent/path".to_string()),
            ..FindInput::default()
        };

        let result = find_files(args).await;
        assert!(matches!(result, Err(FindError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn test_validation_error_empty_pattern() {
        let result = FindTest::new().find_result(FindInput { pattern: String::new(), ..FindInput::default() }).await;
        assert!(matches!(result, Err(FindError::Glob(_))));
    }

    #[tokio::test]
    async fn test_default_path() {
        let args = FindInput { pattern: "**/*.rs".to_string(), path: None, ..FindInput::default() };
        let result = find_files(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_results_are_sorted() {
        let result = FindTest::new().with_file("c.rs").with_file("a.rs").with_file("b.rs").find("**/*.rs").await;
        let sorted: Vec<String> = {
            let mut v = result.matches.clone();
            v.sort();
            v
        };
        assert_eq!(result.matches, sorted);
    }

    struct FindTest {
        temp_dir: TempDir,
    }

    impl FindTest {
        fn new() -> Self {
            Self { temp_dir: TempDir::new().unwrap() }
        }

        fn with_file(self, path: &str) -> Self {
            let file_path = self.temp_dir.path().join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            File::create(file_path).unwrap();
            self
        }

        async fn find(&self, pattern: &str) -> FindOutput {
            self.find_with(FindInput { pattern: pattern.to_string(), ..FindInput::default() }).await
        }

        async fn find_with(&self, input: FindInput) -> FindOutput {
            self.find_result(input).await.unwrap()
        }

        async fn find_result(&self, input: FindInput) -> Result<FindOutput, FindError> {
            find_files(FindInput { path: Some(self.path_string()), ..input }).await
        }

        fn path_string(&self) -> String {
            self.temp_dir.path().to_string_lossy().to_string()
        }
    }
}
