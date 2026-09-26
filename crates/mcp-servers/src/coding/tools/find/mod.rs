use crate::coding::error::FindError;
use crate::coding::tools::glob_filter::{CaseSensitivity, PathGlobMatcher};
use ignore::{WalkBuilder, WalkState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

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

    let path_matcher =
        Arc::new(PathGlobMatcher::new(&args.pattern, CaseSensitivity::from_optional(args.case_insensitive))?);
    let state = Arc::new(FindState::new(args.limit));
    let (result_tx, result_rx) = mpsc::channel();

    let mut walker_builder = WalkBuilder::new(search_root);
    walker_builder.hidden(!args.include_hidden.unwrap_or(false)).git_ignore(true).follow_links(false);

    walker_builder.build_parallel().run(|| {
        let path_matcher = path_matcher.clone();
        let state = state.clone();
        let result_tx = result_tx.clone();

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
                state.push(&result_tx, entry.path().to_string_lossy().to_string());
            }

            if state.limit_reached() { WalkState::Quit } else { WalkState::Continue }
        })
    });

    drop(result_tx);
    let (matches, truncated) = state.results(result_rx);
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

struct FindState {
    match_count: AtomicUsize,
    limit: Option<usize>,
}

impl FindState {
    fn new(limit: Option<usize>) -> Self {
        Self { match_count: AtomicUsize::new(0), limit }
    }

    fn push(&self, result_tx: &mpsc::Sender<String>, path: String) {
        if self.limit_reached() {
            return;
        }

        let index = self.match_count.fetch_add(1, Ordering::Relaxed);
        let capacity = self.capacity();
        if index < capacity {
            let _ = result_tx.send(path);
        }
    }

    fn limit_reached(&self) -> bool {
        self.match_count.load(Ordering::Relaxed) >= self.capacity()
    }

    fn results(&self, result_rx: mpsc::Receiver<String>) -> (Vec<String>, bool) {
        let mut matches: Vec<_> = result_rx.into_iter().collect();
        matches.sort();
        let truncated = self.limit.is_some_and(|limit| matches.len() > limit);
        if let Some(limit) = self.limit {
            matches.truncate(limit);
        }
        (matches, truncated)
    }

    fn capacity(&self) -> usize {
        self.limit.map_or(usize::MAX, |limit| limit.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWorkspace;

    async fn find(workspace: &TestWorkspace, pattern: &str) -> FindOutput {
        find_with(workspace, FindInput { pattern: pattern.to_string(), ..FindInput::default() }).await
    }

    async fn find_with(workspace: &TestWorkspace, input: FindInput) -> FindOutput {
        find_result(workspace, input).await.unwrap()
    }

    async fn find_result(workspace: &TestWorkspace, input: FindInput) -> Result<FindOutput, FindError> {
        find_files(FindInput { path: Some(workspace.root_string()), ..input }).await
    }

    #[tokio::test]
    async fn test_exact_pattern_match() {
        let workspace = TestWorkspace::new().file("test.rs", "");
        let result = find(&workspace, "**/test.rs").await;

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("test.rs"));
    }

    #[tokio::test]
    async fn test_glob_wildcard_pattern() {
        let workspace = TestWorkspace::new()
            .file("test.rs", "")
            .file("main.rs", "")
            .file("lib.rs", "")
            .file("notes.txt", "")
            .file("subdir/nested.rs", "");
        let result = find(&workspace, "**/*.rs").await;

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
        let workspace = TestWorkspace::new().file("justfile", "").file("subdir/justfile", "");
        let result = find(&workspace, "justfile").await;

        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("justfile")));
        assert!(result.matches.iter().any(|p| p.ends_with("subdir/justfile")));
    }

    #[tokio::test]
    async fn test_bare_prefix_glob_matches_basename() {
        let workspace = TestWorkspace::new().file("README.md", "").file("docs/README.adoc", "");
        let result = find(&workspace, "README*").await;
        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("README.md")));
        assert!(result.matches.iter().any(|p| p.ends_with("docs/README.adoc")));
    }

    #[tokio::test]
    async fn test_bare_glob_matches_nested_files() {
        let workspace = TestWorkspace::new().file("tsconfig.base.json", "").file("packages/foo/tsconfig.json", "");
        let result = find(&workspace, "tsconfig*.json").await;
        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|p| p.ends_with("tsconfig.base.json")));
        assert!(result.matches.iter().any(|p| p.ends_with("packages/foo/tsconfig.json")));
    }

    #[tokio::test]
    async fn test_relative_path_pattern_matches_relative_to_search_root() {
        let workspace = TestWorkspace::new().file("crates/example/src/lib.rs", "").file("lib.rs", "");
        let result = find(&workspace, "crates/**/*.rs").await;
        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("crates/example/src/lib.rs"));
    }

    #[tokio::test]
    async fn test_limit_truncates_results() {
        let workspace = TestWorkspace::new().file("one.rs", "").file("two.rs", "").file("three.rs", "");
        let result =
            find_with(&workspace, FindInput { pattern: "*.rs".to_string(), limit: Some(2), ..FindInput::default() })
                .await;

        assert_eq!(result.count, 2);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn test_exact_limit_does_not_report_truncation() {
        let workspace = TestWorkspace::new().file("one.rs", "").file("two.rs", "");
        let result =
            find_with(&workspace, FindInput { pattern: "*.rs".to_string(), limit: Some(2), ..FindInput::default() })
                .await;

        assert_eq!(result.count, 2);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_zero_limit_without_matches_is_not_truncated() {
        let result = find_with(
            &TestWorkspace::new(),
            FindInput { pattern: "*.rs".to_string(), limit: Some(0), ..FindInput::default() },
        )
        .await;

        assert_eq!(result.count, 0);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_limit_not_reached_returns_all_matches() {
        let workspace = TestWorkspace::new().file("one.rs", "").file("two.rs", "");
        let result =
            find_with(&workspace, FindInput { pattern: "*.rs".to_string(), limit: Some(10), ..FindInput::default() })
                .await;

        assert_eq!(result.count, 2);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_zero_limit_detects_truncation_without_returning_matches() {
        let workspace = TestWorkspace::new().file("one.rs", "");
        let result =
            find_with(&workspace, FindInput { pattern: "*.rs".to_string(), limit: Some(0), ..FindInput::default() })
                .await;

        assert_eq!(result.count, 0);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn test_hidden_files_are_skipped_by_default() {
        let workspace = TestWorkspace::new().file(".aether/settings.json", "");
        let result = find(&workspace, "settings.json").await;
        assert_eq!(result.count, 0);
    }

    #[tokio::test]
    async fn test_include_hidden_finds_hidden_files() {
        let workspace = TestWorkspace::new().file(".aether/settings.json", "");
        let result = find_with(
            &workspace,
            FindInput { pattern: "settings.json".to_string(), include_hidden: Some(true), ..FindInput::default() },
        )
        .await;

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with(".aether/settings.json"));
    }

    #[tokio::test]
    async fn test_case_insensitive_matching() {
        let workspace = TestWorkspace::new().file("README.md", "");
        let result = find_with(
            &workspace,
            FindInput { pattern: "readme*".to_string(), case_insensitive: Some(true), ..FindInput::default() },
        )
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
        let result =
            find_result(&TestWorkspace::new(), FindInput { pattern: String::new(), ..FindInput::default() }).await;
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
        let result = find_result(
            &TestWorkspace::new().file("c.rs", "").file("a.rs", "").file("b.rs", ""),
            FindInput { pattern: "**/*.rs".to_string(), ..FindInput::default() },
        )
        .await
        .unwrap();
        let sorted: Vec<String> = {
            let mut v = result.matches.clone();
            v.sort();
            v
        };
        assert_eq!(result.matches, sorted);
    }
}
