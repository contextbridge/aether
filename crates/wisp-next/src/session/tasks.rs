//! Work the UI hands off rather than doing on the event loop, and the results
//! that come back.
//!
//! The event loop drains tasks each frame, runs them on the tokio runtime,
//! and feeds the result back to `App` on the next turn of the loop.

use crate::components::generation::Generation;
use crate::components::theme::Theme;
use crate::screens::git_diff::{GitDiffEvent, GitDiffTask};
use crate::session::workspace_status::{WorkspaceStatus, home_relative_path};
use crate::settings::{UiSettings, list_theme_files, load_theme_file, save_settings};
use crate::surfaces::attachments::{AttachmentOutcome, PromptAttachment, build_attachments};
use crate::surfaces::picker::{FileEntry, index_files};
use std::path::PathBuf;

#[derive(Debug)]
pub enum Task {
    GitDiff(GitDiffTask),
    IndexFiles { request_id: Generation, root: PathBuf },
    PrepareSubmission { attachments: Vec<PromptAttachment> },
    ListThemes,
    ApplyTheme { settings: UiSettings, value: String },
    ResolveWorkspace { cwd: PathBuf },
}

pub enum TaskResult {
    FilesIndexed { request_id: Generation, files: Vec<FileEntry> },
    GitDiff(GitDiffEvent),
    SubmissionPrepared(AttachmentOutcome),
    ThemesListed(Vec<String>),
    ThemeApplied { settings: UiSettings, theme: Theme, error: Option<String> },
    WorkspaceResolved { cwd: PathBuf, status: WorkspaceStatus },
}

impl Task {
    pub async fn execute(self) -> TaskResult {
        match self {
            Self::GitDiff(task) => TaskResult::GitDiff(task.execute().await),
            Self::IndexFiles { request_id, root } => {
                let files = tokio::task::spawn_blocking(move || index_files(&root)).await.unwrap_or_default();
                TaskResult::FilesIndexed { request_id, files }
            }
            Self::PrepareSubmission { attachments } => {
                let outcome =
                    tokio::task::spawn_blocking(move || build_attachments(&attachments)).await.unwrap_or_else(
                        |error| AttachmentOutcome::failed(format!("Could not prepare attachments: {error}")),
                    );
                TaskResult::SubmissionPrepared(outcome)
            }
            Self::ListThemes => {
                TaskResult::ThemesListed(tokio::task::spawn_blocking(list_theme_files).await.unwrap_or_default())
            }
            Self::ApplyTheme { settings, value } => {
                let fallback_settings = settings.clone();
                tokio::task::spawn_blocking(move || {
                    let error = save_settings(&settings).err().map(|error| error.to_string());
                    let theme = if value.is_empty() { Theme::default() } else { load_theme_file(&value) };
                    TaskResult::ThemeApplied { settings, theme, error }
                })
                .await
                .unwrap_or_else(|error| TaskResult::ThemeApplied {
                    settings: fallback_settings,
                    theme: Theme::default(),
                    error: Some(format!("Theme task failed: {error}")),
                })
            }
            Self::ResolveWorkspace { cwd } => {
                let fallback_cwd = cwd.clone();
                tokio::task::spawn_blocking(move || {
                    let status = WorkspaceStatus::resolve(&cwd);
                    TaskResult::WorkspaceResolved { cwd, status }
                })
                .await
                .unwrap_or_else(|_| TaskResult::WorkspaceResolved {
                    status: WorkspaceStatus::new(home_relative_path(&fallback_cwd), None),
                    cwd: fallback_cwd,
                })
            }
        }
    }
}

impl From<GitDiffTask> for Task {
    fn from(task: GitDiffTask) -> Self {
        Self::GitDiff(task)
    }
}
