use crate::coding::error::AstGrepError;
use crate::coding::tools::glob_filter::{CaseSensitivity, PathGlobMatcher, build_path_matcher};
use ast_grep_core::Doc;
use ast_grep_core::matcher::{NodeMatch, Pattern};
use ast_grep_language::{Language, LanguageExt, SupportLang};
use ignore::{WalkBuilder, WalkState};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::task::spawn_blocking;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepInput {
    /// ast-grep pattern code.
    pub pattern: String,
    /// Language alias understood by ast-grep, e.g. "rs", "rust", "ts", "tsx", "py", "js".
    pub language: String,
    /// File or directory to search. Defaults to the coding server workspace root.
    pub path: Option<String>,
    /// Optional glob filter, e.g. "**/*.rs" or "*.{ts,tsx}".
    pub glob: Option<String>,
    /// Lines before each match.
    pub context_before: Option<u32>,
    /// Lines after each match.
    pub context_after: Option<u32>,
    /// Lines before and after each match. Overrides contextBefore/contextAfter.
    pub context_around: Option<u32>,
    /// Maximum number of matches to return.
    pub head_limit: Option<usize>,
    /// Optional regex constraints on metavariable captures.
    /// Key is the metavariable name without `$`; value is a regex the captured text must match.
    /// Example: `{"CRATE": "^crossterm"}` with pattern `use $CRATE;`.
    pub constraints: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepOutput {
    pub matches: Vec<AstGrepMatch>,
    pub count: usize,
    pub truncated: bool,
    pub language: String,
    pub search_path: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepMatch {
    pub file: String,
    pub range: AstGrepRange,
    pub text: String,
    pub captures: Vec<AstGrepCapture>,
    pub before_context: Option<Vec<String>>,
    pub after_context: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepRange {
    /// 1-based line number.
    pub start_line: usize,
    /// 1-based character column.
    pub start_column: usize,
    /// 1-based line number.
    pub end_line: usize,
    /// 1-based character column.
    pub end_column: usize,
    /// 0-based byte offset.
    pub start_byte: usize,
    /// 0-based exclusive byte offset.
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepCapture {
    pub name: String,
    pub text: String,
}

pub async fn perform_ast_grep(mut args: AstGrepInput) -> Result<AstGrepOutput, AstGrepError> {
    spawn_blocking(move || {
        if args.path.as_deref().is_some_and(|p| p.trim().is_empty()) {
            args.path = None;
        }

        let lang: SupportLang =
            args.language.parse().map_err(|_| AstGrepError::UnsupportedLanguage(args.language.clone()))?;

        let pattern = Pattern::try_new(&args.pattern, lang).map_err(|e| AstGrepError::InvalidPattern(e.to_string()))?;

        let constraint_regexes = compile_constraints(args.constraints.as_ref())?;
        let path_matcher = build_path_matcher(args.glob.as_deref(), CaseSensitivity::Sensitive)?;
        let search_path = args.path.as_deref().unwrap_or(".");
        let path = Path::new(search_path);
        if !path.exists() {
            return Err(AstGrepError::PathNotFound(search_path.to_string()));
        }

        let (context_before, context_after) = context_counts(&args);
        let limit = args.head_limit.unwrap_or(usize::MAX);
        let mut matches = if path.is_file() {
            if single_file_matches(path_matcher.as_ref(), path) {
                search_file(path, lang, &pattern, &constraint_regexes, context_before, context_after)?
            } else {
                Vec::new()
            }
        } else {
            search_directory(path, lang, pattern, path_matcher, &constraint_regexes, context_before, context_after)
        };

        matches.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.range.start_byte.cmp(&b.range.start_byte))
                .then(a.range.end_byte.cmp(&b.range.end_byte))
        });
        let truncated = matches.len() > limit;
        matches.truncate(limit);

        let count = matches.len();
        let meta = ToolDisplayMeta::new(
            "AST grep",
            format!("'{}' in {} ({count} matches)", args.pattern, basename(search_path)),
        );

        Ok(AstGrepOutput {
            matches,
            count,
            truncated,
            language: lang.to_string(),
            search_path: search_path.to_string(),
            meta: Some(meta.into()),
        })
    })
    .await
    .map_err(|error| AstGrepError::SearchFailed(error.to_string()))?
}

fn search_directory(
    search_path: &Path,
    lang: SupportLang,
    pattern: Pattern,
    path_matcher: Option<PathGlobMatcher>,
    constraints: &HashMap<String, Regex>,
    context_before: usize,
    context_after: usize,
) -> Vec<AstGrepMatch> {
    let matches = Arc::new(Mutex::new(Vec::new()));
    let pattern = Arc::new(pattern);
    let path_matcher = Arc::new(path_matcher);
    let constraints = Arc::new(constraints.clone());
    let mut walker = WalkBuilder::new(search_path);
    walker.follow_links(false);

    walker.build_parallel().run(|| {
        let matches = matches.clone();
        let pattern = pattern.clone();
        let path_matcher = path_matcher.clone();
        let constraints = constraints.clone();

        Box::new(move |result| {
            let Ok(entry) = result else {
                return WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                return WalkState::Continue;
            }

            if !file_matches_filters(entry.path(), search_path, lang, path_matcher.as_ref().as_ref()) {
                return WalkState::Continue;
            }

            let Ok(file_matches) =
                search_file(entry.path(), lang, &pattern, &constraints, context_before, context_after)
            else {
                return WalkState::Continue;
            };

            if !file_matches.is_empty()
                && let Ok(mut matches) = matches.lock()
            {
                matches.extend(file_matches);
            }

            WalkState::Continue
        })
    });

    matches.lock().map(|matches| matches.clone()).unwrap_or_default()
}

fn file_matches_filters(
    path: &Path,
    search_root: &Path,
    lang: SupportLang,
    path_matcher: Option<&PathGlobMatcher>,
) -> bool {
    <SupportLang as Language>::from_path(path) == Some(lang)
        && path_matcher.is_none_or(|matcher| matcher.matches(path, search_root))
}

fn single_file_matches(path_matcher: Option<&PathGlobMatcher>, path: &Path) -> bool {
    path_matcher.is_none_or(|matcher| matcher.matches(path, path.parent().unwrap_or(path)))
}

fn search_file(
    path: &Path,
    lang: SupportLang,
    pattern: &Pattern,
    constraints: &HashMap<String, Regex>,
    context_before: usize,
    context_after: usize,
) -> Result<Vec<AstGrepMatch>, AstGrepError> {
    let source = read_to_string(path)
        .map_err(|error| AstGrepError::ReadFailed { path: path.display().to_string(), reason: error.to_string() })?;

    let lines: Vec<&str> = source.lines().collect();
    let root = lang.ast_grep(&source);
    let mut matches = Vec::new();
    for node_match in root.root().find_all(pattern.clone()) {
        if !constraints_match(&node_match, constraints) {
            continue;
        }
        matches.push(build_match(path, &lines, &node_match, context_before, context_after));
    }
    Ok(matches)
}

fn build_match<T: Doc>(
    path: &Path,
    lines: &[&str],
    node_match: &NodeMatch<'_, T>,
    context_before: usize,
    context_after: usize,
) -> AstGrepMatch {
    let start = node_match.start_pos();
    let end = node_match.end_pos();
    let range = node_match.range();
    let start_line = start.line() + 1;
    let end_line = end.line() + 1;
    let (before_context, after_context) = context_for_match(lines, start_line, end_line, context_before, context_after);

    AstGrepMatch {
        file: path.to_string_lossy().to_string(),
        range: AstGrepRange {
            start_line,
            start_column: start.column(node_match.get_node()) + 1,
            end_line,
            end_column: end.column(node_match.get_node()) + 1,
            start_byte: range.start,
            end_byte: range.end,
        },
        text: node_match.text().to_string(),
        captures: captures(node_match),
        before_context,
        after_context,
    }
}

fn captures<T: Doc>(node_match: &NodeMatch<'_, T>) -> Vec<AstGrepCapture> {
    let env: HashMap<String, String> = HashMap::from(node_match.get_env().clone());
    let mut captures: Vec<AstGrepCapture> = env.into_iter().map(|(name, text)| AstGrepCapture { name, text }).collect();
    captures.sort_by(|a, b| a.name.cmp(&b.name));
    captures
}

fn context_for_match(
    lines: &[&str],
    start_line: usize,
    end_line: usize,
    before: usize,
    after: usize,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let before_context = if before == 0 {
        None
    } else {
        let start = start_line.saturating_sub(before).max(1);
        let context: Vec<String> =
            lines[start - 1..start_line.saturating_sub(1)].iter().map(|line| (*line).to_string()).collect();
        Some(context)
    };
    let after_context = if after == 0 {
        None
    } else if end_line >= lines.len() {
        Some(Vec::new())
    } else {
        let end = (end_line + after).min(lines.len());
        let context: Vec<String> = lines[end_line..end].iter().map(|line| (*line).to_string()).collect();
        Some(context)
    };
    (before_context, after_context)
}

fn compile_constraints(constraints: Option<&HashMap<String, String>>) -> Result<HashMap<String, Regex>, AstGrepError> {
    let Some(constraints) = constraints else { return Ok(HashMap::new()) };
    constraints
        .iter()
        .map(|(name, pattern)| {
            Regex::new(pattern)
                .map(|regex| (name.clone(), regex))
                .map_err(|error| AstGrepError::InvalidConstraintRegex { name: name.clone(), reason: error.to_string() })
        })
        .collect()
}

fn constraints_match<T: Doc>(node_match: &NodeMatch<'_, T>, constraints: &HashMap<String, Regex>) -> bool {
    let env = node_match.get_env();
    constraints.iter().all(|(name, regex)| env.get_match(name).is_some_and(|node| regex.is_match(&node.text())))
}

fn context_counts(args: &AstGrepInput) -> (usize, usize) {
    if let Some(around) = args.context_around {
        (around as usize, around as usize)
    } else {
        (args.context_before.unwrap_or(0) as usize, args.context_after.unwrap_or(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn input(pattern: &str, path: &Path) -> AstGrepInput {
        AstGrepInput {
            pattern: pattern.to_string(),
            language: "rs".to_string(),
            path: Some(path.to_string_lossy().to_string()),
            glob: None,
            context_before: None,
            context_after: None,
            context_around: None,
            head_limit: None,
            constraints: None,
        }
    }

    #[tokio::test]
    async fn finds_rust_function_pattern() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\nfn bar() { let x = 1; }\n").unwrap();

        let result = perform_ast_grep(input("fn $NAME() { $$$BODY }", temp.path())).await.unwrap();

        assert_eq!(result.count, 2);
        assert!(result.matches.iter().any(|m| m.text == "fn foo() {}"));
        assert!(result.matches.iter().any(|m| m.text == "fn bar() { let x = 1; }"));
    }

    #[tokio::test]
    async fn returns_metavariable_captures() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\n").unwrap();

        let result = perform_ast_grep(input("fn $NAME() {}", temp.path())).await.unwrap();

        assert_eq!(result.matches[0].captures[0].name, "NAME");
        assert_eq!(result.matches[0].captures[0].text, "foo");
    }

    #[tokio::test]
    async fn filters_directory_by_language() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\n").unwrap();
        fs::write(temp.path().join("test.py"), "fn fake() {}\n").unwrap();

        let result = perform_ast_grep(input("fn $NAME() {}", temp.path())).await.unwrap();

        assert_eq!(result.count, 1);
        assert!(result.matches[0].file.ends_with("lib.rs"));
    }

    #[tokio::test]
    async fn applies_glob_filter() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::create_dir(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "fn src_fn() {}\n").unwrap();
        fs::write(temp.path().join("tests/lib.rs"), "fn test_fn() {}\n").unwrap();
        let mut args = input("fn $NAME() {}", temp.path());
        args.glob = Some("src/**/*.rs".to_string());
        let result = perform_ast_grep(args).await.unwrap();
        assert_eq!(result.count, 1);
        assert!(result.matches[0].file.ends_with("src/lib.rs"));
    }

    #[tokio::test]
    async fn invalid_language_returns_error() {
        let temp = TempDir::new().unwrap();
        let mut args = input("fn $NAME() {}", temp.path());
        args.language = "not-a-language".to_string();
        let result = perform_ast_grep(args).await;
        assert!(matches!(result, Err(AstGrepError::UnsupportedLanguage(_))));
    }

    #[tokio::test]
    async fn invalid_pattern_returns_error() {
        let temp = TempDir::new().unwrap();
        let result = perform_ast_grep(input("", temp.path())).await;

        assert!(matches!(result, Err(AstGrepError::InvalidPattern(_))));
    }

    #[tokio::test]
    async fn head_limit_truncates_results() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        let mut args = input("fn $NAME() {}", temp.path());
        args.head_limit = Some(1);
        let result = perform_ast_grep(args).await.unwrap();
        assert_eq!(result.count, 1);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn head_limit_zero_returns_no_matches_but_truncated() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn a() {}\n").unwrap();
        let mut args = input("fn $NAME() {}", temp.path());
        args.head_limit = Some(0);
        let result = perform_ast_grep(args).await.unwrap();
        assert_eq!(result.count, 0);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn nonexistent_path_returns_error() {
        let result = perform_ast_grep(input("fn $NAME() {}", Path::new("/no/such/path/exists"))).await;

        assert!(matches!(result, Err(AstGrepError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn explicit_unreadable_file_returns_error() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("invalid.rs");
        fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        let result = perform_ast_grep(input("fn $NAME() {}", &file)).await;
        assert!(matches!(result, Err(AstGrepError::ReadFailed { .. })));
    }

    #[tokio::test]
    async fn range_is_one_based_and_byte_offsets_are_zero_based() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "\nfn foo() {}\n").unwrap();
        let result = perform_ast_grep(input("fn $NAME() {}", temp.path())).await.unwrap();
        let range = &result.matches[0].range;

        assert_eq!(range.start_line, 2);
        assert_eq!(range.start_column, 1);
        assert_eq!(range.start_byte, 1);
    }

    #[tokio::test]
    async fn directory_search_skips_hidden_files() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("visible.rs"), "fn visible() {}\n").unwrap();
        fs::write(temp.path().join(".hidden.rs"), "fn hidden() {}\n").unwrap();
        let result = perform_ast_grep(input("fn $NAME() {}", temp.path())).await.unwrap();

        assert_eq!(result.count, 1);
        assert_eq!(result.matches[0].text, "fn visible() {}");
    }

    #[tokio::test]
    async fn directory_search_respects_gitignore() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join(".gitignore"), "ignored.rs\n").unwrap();
        fs::write(temp.path().join("included.rs"), "fn included() {}\n").unwrap();
        fs::write(temp.path().join("ignored.rs"), "fn ignored() {}\n").unwrap();

        let result = perform_ast_grep(input("fn $NAME() {}", temp.path())).await.unwrap();

        assert_eq!(result.count, 1);
        assert_eq!(result.matches[0].text, "fn included() {}");
    }

    #[tokio::test]
    async fn includes_context_lines() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "before\nfn foo() {}\nafter\n").unwrap();
        let mut args = input("fn $NAME() {}", temp.path());
        args.context_around = Some(1);

        let result = perform_ast_grep(args).await.unwrap();

        assert_eq!(result.matches[0].before_context.as_ref().unwrap(), &vec!["before".to_string()]);
        assert_eq!(result.matches[0].after_context.as_ref().unwrap(), &vec!["after".to_string()]);
    }

    #[tokio::test]
    async fn filters_matches_by_constraint() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "use crossterm::execute;\nuse serde::Serialize;\n").unwrap();

        let mut args = input("use $CRATE;", temp.path());
        args.constraints = Some(HashMap::from([("CRATE".to_string(), "^crossterm".to_string())]));

        let result = perform_ast_grep(args).await.unwrap();

        assert_eq!(result.count, 1);
        assert_eq!(result.matches[0].text, "use crossterm::execute;");
    }

    #[tokio::test]
    async fn constraint_with_no_matches_returns_empty() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\n").unwrap();

        let mut args = input("fn $NAME() {}", temp.path());
        args.constraints = Some(HashMap::from([("NAME".to_string(), "^bar".to_string())]));

        let result = perform_ast_grep(args).await.unwrap();

        assert_eq!(result.count, 0);
    }

    #[tokio::test]
    async fn constraint_on_missing_capture_returns_empty() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\n").unwrap();

        let mut args = input("fn $NAME() {}", temp.path());
        args.constraints = Some(HashMap::from([("MISSING".to_string(), ".*".to_string())]));

        let result = perform_ast_grep(args).await.unwrap();

        assert_eq!(result.count, 0);
    }

    #[tokio::test]
    async fn invalid_constraint_regex_returns_error() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn foo() {}\n").unwrap();

        let mut args = input("fn $NAME() {}", temp.path());
        args.constraints = Some(HashMap::from([("NAME".to_string(), "(".to_string())]));

        let result = perform_ast_grep(args).await;

        assert!(matches!(result, Err(AstGrepError::InvalidConstraintRegex { .. })));
    }

    #[tokio::test]
    async fn multiple_constraints_all_must_match() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("lib.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();

        let mut args = input("fn $NAME() {}", temp.path());
        args.constraints =
            Some(HashMap::from([("NAME".to_string(), "^a".to_string()), ("NAME".to_string(), "lpha".to_string())]));

        let result = perform_ast_grep(args).await.unwrap();

        assert_eq!(result.count, 1);
        assert!(result.matches[0].text.contains("alpha"));
    }
}
