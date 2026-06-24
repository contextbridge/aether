pub mod common;

use crate::coding::error::GrepError;
use crate::coding::tools::glob_filter::{PathGlobMatcher, build_path_matcher};
use aether_lspd::extensions_for_alias as extensions_for_type;
use common::{CountSink, HasMatchSink, MatchCollectorSink, MatchData, OutputMode};
use grep::{
    regex::{RegexMatcher, RegexMatcherBuilder},
    searcher::{BinaryDetection, Searcher, SearcherBuilder},
};
use ignore::{WalkBuilder, WalkState};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepContentOutput {
    /// Matching lines with context
    pub matches: Vec<MatchData>,
    /// Total number of matches
    pub total_matches: usize,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepFilesOutput {
    /// Files containing matches
    pub files: Vec<String>,
    /// Number of files with matches
    pub count: usize,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepFileCount {
    pub file: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepCountOutput {
    /// Match counts per file
    pub counts: Vec<GrepFileCount>,
    /// Total matches across all files
    pub total: usize,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[schemars(extend("type" = "object"))]
pub enum GrepOutput {
    Content(GrepContentOutput),
    Files(GrepFilesOutput),
    Count(GrepCountOutput),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrepInput {
    /// The regular expression pattern to search for in file contents
    pub pattern: String,
    /// Absolute path to a file or directory to search in. Defaults to the current working directory.
    pub path: Option<String>,
    /// Glob pattern to filter files (e.g. "*.js", "*.{ts,tsx}")
    pub glob: Option<String>,
    /// File type to search (e.g. "js", "py", "rust"). More efficient than glob for standard file types.
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    /// Output mode: "content" (default, shows matching lines), "filesWithMatches" (file paths only), or "count" (match counts per file)
    pub output_mode: Option<OutputMode>,
    /// Case insensitive search
    pub case_insensitive: Option<bool>,
    /// Show line numbers in output (content mode only). Defaults to true.
    pub line_numbers: Option<bool>,
    /// Number of lines to show before each match (content mode only)
    pub context_before: Option<u32>,
    /// Number of lines to show after each match (content mode only)
    pub context_after: Option<u32>,
    /// Number of lines to show before and after each match (content mode only). Overrides contextBefore/contextAfter.
    pub context_around: Option<u32>,
    /// Limit output to first N entries. In content mode limits match lines, in filesWithMatches mode limits file paths, in count mode limits file entries.
    pub head_limit: Option<usize>,
    /// Enable multiline mode where . matches newlines and patterns can span lines
    pub multiline: Option<bool>,
}

/// Thread-safe state for parallel grep execution
struct ParallelGrepState {
    matches: Mutex<Vec<MatchData>>,
    files_with_matches: Mutex<Vec<String>>,
    file_counts: Mutex<Vec<GrepFileCount>>,
    total_items: AtomicUsize,
    max_items: usize,
    limit_reached: AtomicBool,
}

impl ParallelGrepState {
    fn new(max_items: usize) -> Self {
        Self {
            matches: Mutex::new(Vec::new()),
            files_with_matches: Mutex::new(Vec::new()),
            file_counts: Mutex::new(Vec::new()),
            total_items: AtomicUsize::new(0),
            max_items,
            limit_reached: AtomicBool::new(false),
        }
    }
}

fn should_include_file(
    path: &Path,
    search_root: &Path,
    file_type: Option<&String>,
    path_matcher: Option<&PathGlobMatcher>,
) -> bool {
    if let Some(path_matcher) = path_matcher {
        return path_matcher.matches(path, search_root);
    }

    // Check file type filter
    if let Some(ftype) = file_type {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let extensions = extensions_for_type(ftype);
            return extensions.contains(&ext);
        }
        return false;
    }

    true
}

pub async fn perform_grep(mut args: GrepInput) -> Result<GrepOutput, GrepError> {
    if args.path.as_deref().is_some_and(|p| p.trim().is_empty()) {
        args.path = None;
    }

    let path_matcher = build_path_matcher(args.glob.as_deref(), false)?;

    let matcher = build_matcher(&args.pattern, args.case_insensitive, args.multiline)?;

    let output_mode = args.output_mode.unwrap_or(OutputMode::Content);
    let line_numbers = args.line_numbers.unwrap_or(true);
    let searcher_builder = build_searcher(&args);

    let search_path = args.path.as_deref().unwrap_or(".");
    let path_obj = Path::new(search_path);

    if !path_obj.exists() {
        return Err(GrepError::PathNotFound(search_path.to_string()));
    }

    let config = SearchConfig { output_mode, line_numbers, file_type: args.file_type.as_ref() };

    let results = if path_obj.is_file() {
        search_single_file(
            path_obj,
            search_path,
            args.head_limit,
            &matcher,
            path_matcher.as_ref(),
            &config,
            &searcher_builder,
        )?
    } else {
        search_directory(search_path, &args, path_matcher, matcher, &config, &searcher_builder)
    };

    Ok(build_grep_output(results, output_mode, &args.pattern, search_path))
}

/// Common search parameters shared across single-file and directory search paths.
struct SearchConfig<'a> {
    output_mode: OutputMode,
    line_numbers: bool,
    file_type: Option<&'a String>,
}

/// Collected search results across all searched files.
struct SearchResults {
    matches: Vec<MatchData>,
    files_with_matches: Vec<String>,
    file_counts: Vec<GrepFileCount>,
}

impl SearchResults {
    fn empty() -> Self {
        Self { matches: Vec::new(), files_with_matches: Vec::new(), file_counts: Vec::new() }
    }
}

fn build_matcher(
    pattern: &str,
    case_insensitive: Option<bool>,
    multiline: Option<bool>,
) -> Result<RegexMatcher, GrepError> {
    let mut matcher_builder = RegexMatcherBuilder::new();
    matcher_builder.case_insensitive(case_insensitive.unwrap_or(false));

    if multiline.unwrap_or(false) {
        matcher_builder.multi_line(true).dot_matches_new_line(true);
    }

    matcher_builder.build(pattern).map_err(|e| GrepError::InvalidRegex(e.to_string()))
}

fn build_searcher(args: &GrepInput) -> SearcherBuilder {
    let (context_before, context_after) = if let Some(around) = args.context_around {
        (around, around)
    } else {
        (args.context_before.unwrap_or(0), args.context_after.unwrap_or(0))
    };

    let mut builder = SearcherBuilder::new();
    builder
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(args.line_numbers.unwrap_or(true))
        .before_context(context_before as usize)
        .after_context(context_after as usize);

    if args.multiline.unwrap_or(false) {
        builder.multi_line(true);
    }

    builder
}

fn search_single_file(
    path_obj: &Path,
    search_path: &str,
    head_limit: Option<usize>,
    matcher: &RegexMatcher,
    path_matcher: Option<&PathGlobMatcher>,
    config: &SearchConfig<'_>,
    searcher_builder: &SearcherBuilder,
) -> Result<SearchResults, GrepError> {
    if !should_include_file(path_obj, path_obj.parent().unwrap_or(path_obj), config.file_type, path_matcher) {
        return Ok(SearchResults::empty());
    }

    let mut searcher = searcher_builder.build();
    let mut results = SearchResults::empty();

    match config.output_mode {
        OutputMode::Content => {
            let mut sink = MatchCollectorSink::with_max_results(path_obj, config.line_numbers, head_limit);
            searcher.search_path(matcher, path_obj, &mut sink).map_err(|e| GrepError::SearchFailed(e.to_string()))?;
            results.matches = sink.matches;
        }
        OutputMode::FilesWithMatches => {
            let mut sink = HasMatchSink::new();
            searcher.search_path(matcher, path_obj, &mut sink).map_err(|e| GrepError::SearchFailed(e.to_string()))?;
            if sink.has_match {
                results.files_with_matches.push(search_path.to_string());
            }
        }
        OutputMode::Count => {
            let mut sink = CountSink::new();
            searcher.search_path(matcher, path_obj, &mut sink).map_err(|e| GrepError::SearchFailed(e.to_string()))?;
            if sink.count > 0 {
                results.file_counts.push(GrepFileCount { file: search_path.to_string(), count: sink.count });
            }
        }
    }

    Ok(results)
}

fn search_directory(
    search_path: &str,
    args: &GrepInput,
    path_matcher: Option<PathGlobMatcher>,
    matcher: RegexMatcher,
    config: &SearchConfig<'_>,
    searcher_builder: &SearcherBuilder,
) -> SearchResults {
    let walker = WalkBuilder::new(search_path).hidden(false).git_ignore(true).build_parallel();

    let max_items = args.head_limit.unwrap_or(usize::MAX);
    let state = Arc::new(ParallelGrepState::new(max_items));
    let matcher = Arc::new(matcher);
    let path_matcher = Arc::new(path_matcher);
    let file_type = args.file_type.clone();
    let output_mode = config.output_mode;
    let line_numbers = config.line_numbers;
    let search_root = Path::new(search_path);

    walker.run(|| {
        let state = state.clone();
        let matcher = matcher.clone();
        let path_matcher = path_matcher.clone();
        let file_type = file_type.clone();
        let mut thread_searcher = searcher_builder.build();

        Box::new(move |result| {
            let filter =
                FileFilter { path_matcher: path_matcher.as_ref().as_ref(), file_type: file_type.as_ref(), search_root };
            search_directory_entry(result, &state, &matcher, &filter, &mut thread_searcher, output_mode, line_numbers)
        })
    });

    let mut results = SearchResults {
        matches: state.matches.lock().unwrap().clone(),
        files_with_matches: state.files_with_matches.lock().unwrap().clone(),
        file_counts: state.file_counts.lock().unwrap().clone(),
    };

    results.matches.sort_by(|a, b| a.file.cmp(&b.file));
    results.files_with_matches.sort();
    results.file_counts.sort_by(|a, b| a.file.cmp(&b.file));

    if let Some(limit) = args.head_limit {
        results.matches.truncate(limit);
        results.files_with_matches.truncate(limit);
        results.file_counts.truncate(limit);
    }

    results
}

/// Filter criteria for file selection during directory walks.
struct FileFilter<'a> {
    path_matcher: Option<&'a PathGlobMatcher>,
    file_type: Option<&'a String>,
    search_root: &'a Path,
}

fn search_directory_entry(
    result: std::result::Result<ignore::DirEntry, ignore::Error>,
    state: &ParallelGrepState,
    matcher: &RegexMatcher,
    filter: &FileFilter<'_>,
    searcher: &mut Searcher,
    output_mode: OutputMode,
    line_numbers: bool,
) -> WalkState {
    if state.limit_reached.load(Ordering::Relaxed) {
        return WalkState::Quit;
    }

    let Ok(entry) = result else {
        return WalkState::Continue;
    };

    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
        return WalkState::Continue;
    }

    if !should_include_file(entry.path(), filter.search_root, filter.file_type, filter.path_matcher) {
        return WalkState::Continue;
    }

    match output_mode {
        OutputMode::Content => {
            let mut sink = MatchCollectorSink::with_max_results(entry.path(), line_numbers, None);
            if searcher.search_path(matcher, entry.path(), &mut sink).is_ok() && !sink.matches.is_empty() {
                let new_count = state.total_items.fetch_add(sink.matches.len(), Ordering::SeqCst) + sink.matches.len();
                if new_count >= state.max_items {
                    state.limit_reached.store(true, Ordering::Release);
                }
                if let Ok(mut matches) = state.matches.lock() {
                    matches.extend(sink.matches);
                }
            }
        }
        OutputMode::FilesWithMatches => {
            let mut sink = HasMatchSink::new();
            if searcher.search_path(matcher, entry.path(), &mut sink).is_ok() && sink.has_match {
                let new_count = state.total_items.fetch_add(1, Ordering::SeqCst) + 1;
                if new_count >= state.max_items {
                    state.limit_reached.store(true, Ordering::Release);
                }
                if let Ok(mut files) = state.files_with_matches.lock() {
                    files.push(entry.path().to_string_lossy().to_string());
                }
            }
        }
        OutputMode::Count => {
            let mut sink = CountSink::new();
            if searcher.search_path(matcher, entry.path(), &mut sink).is_ok() && sink.count > 0 {
                let new_count = state.total_items.fetch_add(1, Ordering::SeqCst) + 1;
                if new_count >= state.max_items {
                    state.limit_reached.store(true, Ordering::Release);
                }
                if let Ok(mut counts) = state.file_counts.lock() {
                    counts.push(GrepFileCount { file: entry.path().to_string_lossy().to_string(), count: sink.count });
                }
            }
        }
    }

    WalkState::Continue
}

fn build_grep_output(results: SearchResults, output_mode: OutputMode, pattern: &str, search_path: &str) -> GrepOutput {
    let match_count = match output_mode {
        OutputMode::Content => results.matches.len(),
        OutputMode::FilesWithMatches => results.files_with_matches.len(),
        OutputMode::Count => results.file_counts.iter().map(|fc| fc.count).sum(),
    };

    let display_meta =
        ToolDisplayMeta::new("Grep", format!("'{}' in {} ({} matches)", pattern, basename(search_path), match_count));
    let meta = Some(display_meta.into());

    match output_mode {
        OutputMode::Content => {
            GrepOutput::Content(GrepContentOutput { matches: results.matches, total_matches: match_count, meta })
        }
        OutputMode::FilesWithMatches => {
            GrepOutput::Files(GrepFilesOutput { files: results.files_with_matches, count: match_count, meta })
        }
        OutputMode::Count => {
            GrepOutput::Count(GrepCountOutput { counts: results.file_counts, total: match_count, meta })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn input(pattern: &str) -> GrepInput {
        GrepInput {
            pattern: pattern.to_string(),
            path: None,
            glob: None,
            file_type: None,
            output_mode: None,
            case_insensitive: None,
            line_numbers: None,
            context_before: None,
            context_after: None,
            context_around: None,
            head_limit: None,
            multiline: None,
        }
    }

    fn create_test_dir() -> TempDir {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let p = dir.path();
        fs::write(p.join("test.rs"), "fn main() {\n    println!(\"Hello, world!\");\n    let x = 42;\n}").unwrap();
        fs::write(p.join("script.py"), "def hello():\n    print(\"Hello, world!\")\n    x = 42\n").unwrap();
        fs::write(p.join("app.js"), "function hello() {\n    console.log(\"Hello, world!\");\n    const x = 42;\n}")
            .unwrap();
        dir
    }

    fn unwrap_content(output: GrepOutput) -> GrepContentOutput {
        match output {
            GrepOutput::Content(c) => c,
            other => panic!("Expected Content, got {other:?}"),
        }
    }

    fn unwrap_files(output: GrepOutput) -> GrepFilesOutput {
        match output {
            GrepOutput::Files(f) => f,
            other => panic!("Expected Files, got {other:?}"),
        }
    }

    fn unwrap_count(output: GrepOutput) -> GrepCountOutput {
        match output {
            GrepOutput::Count(c) => c,
            other => panic!("Expected Count, got {other:?}"),
        }
    }

    async fn grep(args: GrepInput) -> GrepOutput {
        perform_grep(args).await.expect("grep failed")
    }

    #[tokio::test]
    async fn test_file_type_and_glob_filtering() {
        let temp_dir = create_test_dir();
        let path = temp_dir.path().to_str().unwrap().to_string();

        let cases: Vec<(Option<String>, Option<String>, &str)> =
            vec![(Some("rust".into()), None, "test.rs"), (None, Some("*.py".into()), "script.py")];
        for (file_type, glob, expected_file) in cases {
            let mut args = input("hello");
            args.path = Some(path.clone());
            args.file_type = file_type;
            args.glob = glob;
            args.output_mode = Some(OutputMode::Content);
            args.case_insensitive = Some(true);
            args.line_numbers = Some(true);

            let content = unwrap_content(grep(args).await);
            assert!(content.total_matches > 0, "no matches for {expected_file}");
            assert!(
                content.matches.iter().all(|m| m.file.contains(expected_file)),
                "expected all matches in {expected_file}"
            );
        }
    }

    #[tokio::test]
    async fn test_files_with_matches_output() {
        let temp_dir = create_test_dir();
        let mut args = input("hello");
        args.path = Some(temp_dir.path().to_str().unwrap().to_string());
        args.output_mode = Some(OutputMode::FilesWithMatches);
        args.case_insensitive = Some(true);

        let files = unwrap_files(grep(args).await);
        assert!(files.count >= 2);
        assert!(files.files.iter().any(|f| f.contains(".py")));
        assert!(files.files.iter().any(|f| f.contains(".js")));
    }

    #[tokio::test]
    async fn test_count_output() {
        let temp_dir = create_test_dir();
        let mut args = input("hello");
        args.path = Some(temp_dir.path().to_str().unwrap().to_string());
        args.output_mode = Some(OutputMode::Count);
        args.case_insensitive = Some(true);

        let count = unwrap_count(grep(args).await);
        assert!(count.counts.len() >= 2);
        assert!(count.total >= 2);
        assert!(count.counts.iter().all(|fc| fc.count > 0));
    }

    #[tokio::test]
    async fn test_head_limit() {
        let temp_dir = create_test_dir();
        let mut args = input("hello");
        args.path = Some(temp_dir.path().to_str().unwrap().to_string());
        args.output_mode = Some(OutputMode::Content);
        args.case_insensitive = Some(true);
        args.line_numbers = Some(true);
        args.head_limit = Some(1);

        let content = unwrap_content(grep(args).await);
        assert!(content.total_matches <= 1);
    }

    #[tokio::test]
    async fn test_multiline_mode() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let file = temp_dir.path().join("multiline.txt");
        fs::write(&file, "start\nmiddle content\nend").unwrap();

        let mut args = input(r"start.*end");
        args.path = Some(file.to_str().unwrap().to_string());
        args.output_mode = Some(OutputMode::Content);
        args.line_numbers = Some(true);
        args.multiline = Some(true);

        let content = unwrap_content(grep(args).await);
        assert!(content.total_matches > 0);
    }

    #[tokio::test]
    async fn test_context_lines() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let file = temp_dir.path().join("context.txt");
        fs::write(&file, "line 1\nline 2\ntarget\nline 4\nline 5").unwrap();

        let mut args = input("target");
        args.path = Some(file.to_str().unwrap().to_string());
        args.output_mode = Some(OutputMode::Content);
        args.line_numbers = Some(true);
        args.context_before = Some(1);
        args.context_after = Some(1);

        let content = unwrap_content(grep(args).await);
        assert!(content.total_matches > 0);
        let m = &content.matches[0];
        let before = m.before_context.as_ref().expect("missing before_context");
        let after = m.after_context.as_ref().expect("missing after_context");
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn grep_input_accepts_camel_case_fields() {
        let args: GrepInput = serde_json::from_value(serde_json::json!({
            "pattern": "hello",
            "path": "/tmp",
            "type": "rust",
            "outputMode": "filesWithMatches",
            "caseInsensitive": true,
            "lineNumbers": true,
            "contextBefore": 1,
            "contextAfter": 2,
            "contextAround": 3,
            "headLimit": 10,
            "multiline": false
        }))
        .unwrap();

        assert_eq!(args.file_type, Some("rust".to_string()));
        assert!(matches!(args.output_mode, Some(OutputMode::FilesWithMatches)));
        assert_eq!(args.case_insensitive, Some(true));
        assert_eq!(args.line_numbers, Some(true));
        assert_eq!(args.context_before, Some(1));
        assert_eq!(args.context_after, Some(2));
        assert_eq!(args.context_around, Some(3));
        assert_eq!(args.head_limit, Some(10));
        assert_eq!(args.multiline, Some(false));
    }

    #[test]
    fn output_mode_files_with_matches_snake_case_alias() {
        let args: GrepInput = serde_json::from_value(serde_json::json!({
            "pattern": "hello",
            "outputMode": "files_with_matches"
        }))
        .unwrap();
        assert!(matches!(args.output_mode, Some(OutputMode::FilesWithMatches)));
    }

    #[tokio::test]
    async fn empty_path_treated_as_cwd() {
        let temp_dir = create_test_dir();
        let _guard = std::env::set_current_dir(temp_dir.path());

        let mut args = input("hello");
        args.path = Some(String::new());
        args.output_mode = Some(OutputMode::Content);
        args.case_insensitive = Some(true);

        assert!(perform_grep(args).await.is_ok());
    }

    #[tokio::test]
    async fn nonexistent_path_returns_error() {
        let mut args = input("hello");
        args.path = Some("/no/such/path/exists".to_string());

        let err = perform_grep(args).await.unwrap_err();
        assert!(matches!(err, GrepError::PathNotFound(_)), "Expected PathNotFound, got: {err}");
    }
}
