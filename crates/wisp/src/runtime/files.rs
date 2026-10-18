use crate::attachment::{AttachmentOutcome, build_attachments};
use crate::command::{CommandResult, FilesystemCommand};
use crate::file_index::index_files;
use crate::settings::{list_theme_files, review_theme_choices, save_settings};
use crate::theme::{Theme, ThemeApplicationError};
use tokio::task::spawn_blocking;

pub(super) async fn execute(command: FilesystemCommand) -> CommandResult {
    match command {
        FilesystemCommand::IndexFiles { request_id, root } => {
            let indexed = |files| CommandResult::FilesIndexed { request_id, files };
            run_or_fail("Failed to index files", move || index_files(&root), indexed).await
        }
        FilesystemCommand::PrepareSubmission { attachments } => {
            let outcome = spawn_blocking(move || build_attachments(&attachments)).await.unwrap_or_else(|error| {
                AttachmentOutcome {
                    blocks: Vec::new(),
                    warnings: vec![format!("Could not prepare attachments: {error}")],
                }
            });
            CommandResult::SubmissionPrepared(outcome)
        }
        FilesystemCommand::ListThemes => {
            run_or_fail("Failed to list themes", list_theme_files, CommandResult::ThemesListed).await
        }
        FilesystemCommand::ListReviewThemes => {
            run_or_fail("Failed to load review themes", review_theme_choices, CommandResult::ReviewThemesListed).await
        }
        FilesystemCommand::ApplyTheme { settings } => CommandResult::ThemeApplied(
            spawn_blocking(move || {
                let theme = Theme::load_selection(&settings.theme)?;
                save_settings(&settings).map_err(ThemeApplicationError::Save)?;
                Ok((settings, theme))
            })
            .await
            .map_err(ThemeApplicationError::Task)
            .and_then(std::convert::identity),
        ),
    }
}

async fn run_or_fail<T: Send + 'static>(
    label: &str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> CommandResult,
) -> CommandResult {
    match spawn_blocking(work).await {
        Ok(value) => done(value),
        Err(error) => CommandResult::BackgroundFailed(format!("{label}: {error}")),
    }
}
