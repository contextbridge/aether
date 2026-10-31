use super::attachment::PromptAttachment;
use clankerdiff_client::{ClientState, ConnectionState};
use clankerdiff_core::RepoPath;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FileListing {
    #[default]
    Pending,
    Listed(RepoFiles),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFiles {
    pub root: PathBuf,
    pub paths: Arc<[RepoPath]>,
}

impl FileListing {
    pub fn of(review: Option<&ClientState>) -> Self {
        let Some(review) = review else {
            return Self::Pending;
        };
        match (&review.snapshot, &review.files, &review.connection, &review.error) {
            (Some(snapshot), Some(paths), ..) => {
                Self::Listed(RepoFiles { root: PathBuf::from(snapshot.repo_root.as_str()), paths: paths.clone() })
            }
            (.., ConnectionState::Failed(error), _) => Self::Failed(error.to_string()),
            (None, _, _, Some(error)) => Self::Failed(error.to_string()),
            _ => Self::Pending,
        }
    }

    pub fn into_result(self) -> Result<RepoFiles, String> {
        match self {
            Self::Listed(files) => Ok(files),
            Self::Failed(reason) => Err(format!("Couldn't list files: {reason}")),
            Self::Pending => Err("Loading files…".to_owned()),
        }
    }
}

impl RepoFiles {
    pub fn attachment(&self, path: &RepoPath) -> PromptAttachment {
        PromptAttachment::in_workspace(self.root.join(path.as_str()), &self.root)
    }

    pub fn repo_path(&self, path: &Path) -> Option<RepoPath> {
        RepoPath::new(path.strip_prefix(&self.root).ok()?.to_str()?).ok()
    }
}
