use futures::future::join_all;
use serde::de::DeserializeOwned;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Represents a parsed markdown file with optional frontmatter
#[derive(Debug, Clone)]
pub struct MarkdownFile<T: DeserializeOwned> {
    /// Parsed frontmatter (if present)
    pub frontmatter: Option<T>,
    /// The content after frontmatter
    pub content: String,
}

impl<T: DeserializeOwned + Send + 'static> MarkdownFile<T> {
    pub fn parse(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let raw_content = fs::read_to_string(path)?;

        match split_frontmatter(&raw_content) {
            Some((yaml_str, body)) => {
                let frontmatter = serde_yml::from_str(yaml_str).ok();
                Ok(Self { frontmatter, content: body.to_string() })
            }
            None => Ok(Self { frontmatter: None, content: raw_content.trim().to_string() }),
        }
    }

    /// List all markdown files in a directory
    pub fn list(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, io::Error> {
        let paths: Vec<_> = fs::read_dir(dir)?
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                (path.extension().and_then(|s| s.to_str()) == Some("md")).then_some(path)
            })
            .collect();

        Ok(paths)
    }

    /// Load a single markdown file from a path
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ParseError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("File not found: {}", path.display()),
            )));
        }

        Self::parse(path)
    }

    /// Load all markdown files from a directory
    pub async fn from_dir(dir: &PathBuf) -> Result<Vec<(PathBuf, Self)>, io::Error> {
        if !dir.exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("Directory not found: {}", dir.display())));
        }

        if !dir.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("Not a directory: {}", dir.display())));
        }

        let parse_tasks: Vec<_> = Self::list(dir)?
            .into_iter()
            .map(|path| {
                tokio::spawn(async move {
                    let path_clone = path.clone();
                    Self::parse(path).map(|f| (path_clone, f))
                })
            })
            .collect();

        let results = join_all(parse_tasks).await;
        let items = results
            .into_iter()
            .filter_map(|result| match result {
                Ok(Ok(item)) => Some(item),
                Ok(Err(e)) => {
                    tracing::warn!("Failed to parse file: {}", e);
                    None
                }
                Err(_) => None,
            })
            .collect();

        Ok(items)
    }

    /// Load all markdown files from nested subdirectories, where each subdirectory
    /// contains a file with the specified filename.
    ///
    /// Flat files in the parent directory are ignored. Only subdirectories containing
    /// the specified filename are processed.
    ///
    /// # Example
    /// ```ignore
    /// // Load from:
    /// //   skills/skill-1/SKILL.md
    /// //   skills/skill-2/SKILL.md
    /// //   skills/flat-file.md      -> ignored (not in a subdirectory)
    /// let skills = MarkdownFile::from_nested_dirs(Path::new("skills"), "SKILL.md").await?;
    /// ```
    pub async fn from_nested_dirs(
        parent_dir: impl AsRef<Path>,
        filename: &str,
    ) -> Result<Vec<(PathBuf, Self)>, io::Error> {
        let parent_dir = parent_dir.as_ref();

        if !parent_dir.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Directory not found: {}", parent_dir.display()),
            ));
        }

        if !parent_dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("Not a directory: {}", parent_dir.display()),
            ));
        }

        let subdirs = list_subdirs(parent_dir)?;
        let filename = filename.to_string();
        let parse_tasks: Vec<_> = subdirs
            .into_iter()
            .map(|dir| {
                let filename = filename.clone();
                tokio::spawn(async move {
                    let file_path = dir.join(&filename);
                    Self::parse(&file_path).map(|f| (dir, f))
                })
            })
            .collect();

        let results = join_all(parse_tasks).await;
        let items = results
            .into_iter()
            .filter_map(|result| match result {
                Ok(Ok(item)) => Some(item),
                Ok(Err(e)) => {
                    tracing::debug!("Skipping directory: {}", e);
                    None
                }
                Err(_) => None,
            })
            .collect();

        Ok(items)
    }
}

/// Split YAML frontmatter from markdown content.
///
/// Returns `(yaml_str, body)` if frontmatter delimiters (`---`) are found,
/// or `None` if the content has no frontmatter.
pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim();
    let rest = content.strip_prefix("---")?;
    let end_pos = rest.find("\n---")?;
    Some((&rest[..end_pos], rest[end_pos + 4..].trim()))
}

/// List all subdirectories in a directory
fn list_subdirs(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, io::Error> {
    let paths: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.is_dir().then_some(path)
        })
        .collect();

    Ok(paths)
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid filename")]
    InvalidFilename,
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}
