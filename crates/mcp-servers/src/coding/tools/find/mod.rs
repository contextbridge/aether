use glob::Pattern;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::coding::error::FindError;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindInput {
    /// The glob pattern to match files against
    pub pattern: String,
    /// The directory to search in (defaults to cwd)
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindOutput {
    /// Array of matching file paths
    pub matches: Vec<String>,
    /// Number of matches found
    pub count: usize,
    /// Search directory used
    pub search_path: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn find_files_by_name(args: FindInput) -> Result<FindOutput, FindError> {
    let search_path = args.path.as_deref().unwrap_or(".");

    // Validate path exists
    if !Path::new(search_path).exists() {
        return Err(FindError::PathNotFound(search_path.to_string()));
    }

    // Compile glob pattern
    let glob_pattern = match Pattern::new(&args.pattern) {
        Ok(pattern) => pattern,
        Err(e) => {
            return Err(FindError::InvalidGlobPattern { pattern: args.pattern, reason: e.to_string() });
        }
    };

    // Build walker with configuration
    let mut walker_builder = WalkBuilder::new(search_path);
    walker_builder.hidden(false).git_ignore(true).follow_links(false);

    // Use parallel walker for better performance
    let walker = walker_builder.build_parallel();
    let matching_files = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let matching_files_clone = matching_files.clone();
    let glob_pattern_clone = std::sync::Arc::new(glob_pattern);

    walker.run(|| {
        let matching_files = matching_files_clone.clone();
        let glob_pattern = glob_pattern_clone.clone();

        Box::new(move |result| {
            if let Ok(entry) = result {
                let path = entry.path();

                // Only process files (not directories)
                if let Some(file_type) = entry.file_type()
                    && !file_type.is_file()
                {
                    return ignore::WalkState::Continue;
                }

                // Check glob pattern matching against the full path
                let path_str = path.to_string_lossy();
                if glob_pattern.matches(&path_str)
                    && let Ok(mut files) = matching_files.lock()
                {
                    files.push(path_str.to_string());
                }
            }
            ignore::WalkState::Continue
        })
    });

    // Extract results - wait for all threads to complete first
    drop(matching_files_clone);
    drop(glob_pattern_clone);

    // Extract results using the remaining Arc reference
    let mut matches = matching_files.lock().map_err(|_| FindError::LockFailed)?.clone();

    // Sort results by path
    matches.sort();

    let count = matches.len();

    let display_meta = ToolDisplayMeta::new("Find files", format!("'{}' ({count} files)", args.pattern));

    Ok(FindOutput { matches, count, search_path: search_path.to_string(), meta: Some(display_meta.into()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::path::Path;
    use tempfile::TempDir;

    fn create_test_files(temp_dir: &Path) -> Result<(), std::io::Error> {
        File::create(temp_dir.join("test.rs"))?;
        File::create(temp_dir.join("main.rs"))?;
        File::create(temp_dir.join("lib.rs"))?;
        File::create(temp_dir.join("example.txt"))?;
        File::create(temp_dir.join("README.md"))?;

        // Create subdirectory with files
        let sub_dir = temp_dir.join("subdir");
        fs::create_dir(&sub_dir)?;
        File::create(sub_dir.join("nested.rs"))?;
        File::create(sub_dir.join("other.txt"))?;

        Ok(())
    }

    #[tokio::test]
    async fn test_exact_pattern_match() {
        let temp_dir = TempDir::new().unwrap();
        create_test_files(temp_dir.path()).unwrap();

        let args =
            FindInput { pattern: "**/test.rs".to_string(), path: Some(temp_dir.path().to_string_lossy().to_string()) };

        let result = find_files_by_name(args).await.unwrap();

        assert_eq!(result.count, 1);
        assert!(result.matches[0].ends_with("test.rs"));
    }

    #[tokio::test]
    async fn test_glob_wildcard_pattern() {
        let temp_dir = TempDir::new().unwrap();
        create_test_files(temp_dir.path()).unwrap();

        let args =
            FindInput { pattern: "**/*.rs".to_string(), path: Some(temp_dir.path().to_string_lossy().to_string()) };

        let result = find_files_by_name(args).await.unwrap();

        // Should find all .rs files including nested ones
        assert!(result.count >= 4); // test.rs, main.rs, lib.rs, nested.rs
        assert!(
            result
                .matches
                .iter()
                .all(|p| { std::path::Path::new(p).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("rs")) })
        );
    }

    #[tokio::test]
    async fn test_validation_error_invalid_path() {
        let args = FindInput { pattern: "**/*.rs".to_string(), path: Some("/nonexistent/path".to_string()) };

        let result = find_files_by_name(args).await;
        assert!(matches!(result, Err(FindError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn test_default_path() {
        // Test that path defaults to current directory
        let args = FindInput { pattern: "**/*.rs".to_string(), path: None };

        let result = find_files_by_name(args).await;
        // Should not error, even if no matches found
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_results_are_sorted() {
        let temp_dir = TempDir::new().unwrap();
        create_test_files(temp_dir.path()).unwrap();

        let args =
            FindInput { pattern: "**/*.rs".to_string(), path: Some(temp_dir.path().to_string_lossy().to_string()) };

        let result = find_files_by_name(args).await.unwrap();

        // Verify results are sorted
        let sorted: Vec<String> = {
            let mut v = result.matches.clone();
            v.sort();
            v
        };
        assert_eq!(result.matches, sorted);
    }
}
