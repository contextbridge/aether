use crate::coding::error::GlobError;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Whether glob matching distinguishes uppercase from lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseSensitivity {
    /// Match patterns exactly, respecting case.
    Sensitive,
    /// Match patterns ignoring case differences.
    Insensitive,
}

impl CaseSensitivity {
    /// Map a nullable "case-insensitive" flag (e.g. from a tool input) onto a
    /// concrete sensitivity, treating `None`/`Some(false)` as case-sensitive.
    pub fn from_optional(case_insensitive: Option<bool>) -> Self {
        if case_insensitive.unwrap_or(false) { Self::Insensitive } else { Self::Sensitive }
    }

    fn is_insensitive(self) -> bool {
        matches!(self, Self::Insensitive)
    }
}

#[derive(Debug, Clone)]
pub struct PathGlobMatcher {
    matcher: GlobSet,
    kind: PathGlobKind,
}

#[derive(Debug, Clone, Copy)]
enum PathGlobKind {
    Basename,
    RelativePath,
}

pub fn build_path_matcher(
    glob: Option<&str>,
    case_sensitivity: CaseSensitivity,
) -> Result<Option<PathGlobMatcher>, GlobError> {
    glob.map(|glob| PathGlobMatcher::new(glob, case_sensitivity)).transpose()
}

impl PathGlobMatcher {
    pub fn new(pattern: &str, case_sensitivity: CaseSensitivity) -> Result<Self, GlobError> {
        if pattern.is_empty() {
            return Err(GlobError::InvalidPattern {
                pattern: pattern.to_string(),
                reason: "pattern cannot be empty".to_string(),
            });
        }

        let kind = if contains_separator(pattern) { PathGlobKind::RelativePath } else { PathGlobKind::Basename };
        let mut builder = GlobSetBuilder::new();
        add_glob(&mut builder, pattern, case_sensitivity)?;

        // globset's `**/` requires at least one leading directory, so `**/*.rs`
        // on its own misses files at the search root. Also register the
        // `**/`-stripped pattern so depth-zero matches (e.g. `lib.rs`) are found.
        if let Some(root_pattern) = pattern.strip_prefix("**/")
            && !root_pattern.is_empty()
        {
            add_glob(&mut builder, root_pattern, case_sensitivity)?;
        }

        Ok(Self { matcher: builder.build().map_err(GlobError::BuildFailed)?, kind })
    }

    pub fn matches(&self, path: &Path, search_root: &Path) -> bool {
        match self.kind {
            PathGlobKind::Basename => path.file_name().is_some_and(|name| self.matcher.is_match(name)),
            PathGlobKind::RelativePath => {
                path.strip_prefix(search_root).is_ok_and(|relative| self.matcher.is_match(relative))
            }
        }
    }
}

fn add_glob(builder: &mut GlobSetBuilder, pattern: &str, case_sensitivity: CaseSensitivity) -> Result<(), GlobError> {
    let glob = GlobBuilder::new(pattern)
        .case_insensitive(case_sensitivity.is_insensitive())
        .build()
        .map_err(|e| GlobError::InvalidPattern { pattern: pattern.to_string(), reason: e.to_string() })?;
    builder.add(glob);
    Ok(())
}

fn contains_separator(pattern: &str) -> bool {
    pattern.contains('/') || pattern.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_patterns_match_basenames_at_any_depth() {
        let matcher = PathGlobMatcher::new("README*", CaseSensitivity::Sensitive).unwrap();
        let root = Path::new("/workspace");

        assert!(matcher.matches(Path::new("/workspace/README.md"), root));
        assert!(matcher.matches(Path::new("/workspace/docs/README.adoc"), root));
        assert!(!matcher.matches(Path::new("/workspace/docs/guide.md"), root));
    }

    #[test]
    fn slash_patterns_match_relative_paths_only() {
        let matcher = PathGlobMatcher::new("crates/**/*.rs", CaseSensitivity::Sensitive).unwrap();
        let root = Path::new("/workspace");

        assert!(matcher.matches(Path::new("/workspace/crates/app/src/lib.rs"), root));
        assert!(!matcher.matches(Path::new("/workspace/examples/demo.rs"), root));
        assert!(!matcher.matches(Path::new("/parent/crates/app/src/lib.rs"), root));
    }

    #[test]
    fn recursive_slash_patterns_match_root_files() {
        let matcher = PathGlobMatcher::new("**/*.rs", CaseSensitivity::Sensitive).unwrap();
        let root = Path::new("/workspace");

        assert!(matcher.matches(Path::new("/workspace/lib.rs"), root));
        assert!(matcher.matches(Path::new("/workspace/src/main.rs"), root));
    }

    #[test]
    fn case_insensitive_patterns_match_basenames() {
        let matcher = PathGlobMatcher::new("readme*", CaseSensitivity::Insensitive).unwrap();
        assert!(matcher.matches(Path::new("/workspace/README.md"), Path::new("/workspace")));
    }
}
