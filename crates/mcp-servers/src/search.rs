//! Shared source-file search utilities.

use std::path::{Path, PathBuf};

use grep::{
    regex::RegexMatcherBuilder,
    searcher::{Searcher, Sink, SinkMatch},
};
use ignore::WalkBuilder;
use thiserror::Error;

/// Errors that can occur while searching project source files
#[derive(Debug, Error)]
pub enum SearchError {
    #[error("Failed to build search matcher: {0}")]
    InvalidMatcher(#[source] grep::regex::Error),

    #[error("Search task failed: {0}")]
    TaskFailed(#[source] tokio::task::JoinError),
}

/// Node package installation directory.
pub const NODE_MODULES: &str = "node_modules";
/// pnpm package store directory.
pub const PNPM_STORE: &str = ".pnpm";
/// Directory names belonging to dependencies or build output rather than project source.
pub const DEPENDENCY_DIRS: &[&str] = &[NODE_MODULES, PNPM_STORE, "target"];

/// Find project source files whose contents contain `literal`, limited to `extensions`.
pub async fn find_files_containing(
    root: PathBuf,
    literal: String,
    extensions: Vec<&'static str>,
    max_files: usize,
) -> Result<Vec<PathBuf>, SearchError> {
    tokio::task::spawn_blocking(move || find_files_containing_sync(&root, &literal, &extensions, max_files))
        .await
        .map_err(SearchError::TaskFailed)?
}

fn find_files_containing_sync(
    root: &Path,
    literal: &str,
    extensions: &[&str],
    max_files: usize,
) -> Result<Vec<PathBuf>, SearchError> {
    if max_files == 0 {
        return Ok(Vec::new());
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(true)
        .build(&regex::escape(literal))
        .map_err(SearchError::InvalidMatcher)?;
    let mut walker = WalkBuilder::new(root);
    walker.hidden(false).git_ignore(true).filter_entry(|entry| {
        entry.depth() == 0
            || entry.file_name().to_str().is_none_or(|name| name != ".git" && !DEPENDENCY_DIRS.contains(&name))
    });

    let mut files = Vec::new();
    let mut searcher = Searcher::new();
    for entry in walker.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || !has_extension(entry.path(), extensions) {
            continue;
        }
        let mut sink = HasMatch::default();
        if searcher.search_path(&matcher, entry.path(), &mut sink).is_ok() && sink.0 {
            files.push(entry.into_path());
            if files.len() == max_files {
                break;
            }
        }
    }
    files.sort();
    Ok(files)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extensions.contains(&extension))
}

#[derive(Default)]
struct HasMatch(bool);

impl Sink for HasMatch {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, _: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.0 = true;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn excludes_dependencies_and_filters_extensions() {
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("component.tsx"), "export const Needle = 1;").unwrap();
        fs::write(directory.path().join("ignored.rs"), "Needle").unwrap();
        fs::create_dir(directory.path().join("node_modules")).unwrap();
        fs::write(directory.path().join("node_modules/ignored.tsx"), "Needle").unwrap();

        let files = find_files_containing(directory.path().to_path_buf(), "needle".to_string(), vec!["ts", "tsx"], 100)
            .await
            .unwrap();

        assert_eq!(files, vec![directory.path().join("component.tsx")]);
    }
}
