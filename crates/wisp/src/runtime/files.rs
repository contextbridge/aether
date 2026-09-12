use crate::attachment::{AttachmentOutcome, build_attachments};
use crate::command::{CommandResult, FailedCommand, FilesystemCommand};
use crate::file_index::index_files;
use crate::settings::{list_theme_files, review_theme_choices, save_settings};
use crate::theme::{Theme, ThemeApplicationError};
use tokio::task::JoinError;

pub(super) async fn execute(command: FilesystemCommand) -> CommandResult {
    match command {
        FilesystemCommand::IndexFiles { request_id, root } => match run_blocking(move || index_files(&root)).await {
            Ok(files) => CommandResult::FilesIndexed { request_id, files },
            Err(error) => {
                CommandResult::Failed { command: FailedCommand::Other("index files"), error: error.to_string() }
            }
        },
        FilesystemCommand::PrepareSubmission { attachments } => {
            let outcome =
                run_blocking(move || build_attachments(&attachments)).await.unwrap_or_else(|error| AttachmentOutcome {
                    blocks: Vec::new(),
                    placeholders: Vec::new(),
                    warnings: vec![format!("Could not prepare attachments: {error}")],
                });
            CommandResult::SubmissionPrepared(outcome)
        }
        FilesystemCommand::ListThemes => match run_blocking(list_theme_files).await {
            Ok(files) => CommandResult::ThemesListed(files),
            Err(error) => {
                CommandResult::Failed { command: FailedCommand::Other("list themes"), error: error.to_string() }
            }
        },
        FilesystemCommand::ListReviewThemes => match run_blocking(review_theme_choices).await {
            Ok(choices) => CommandResult::ReviewThemesListed(choices),
            Err(error) => {
                CommandResult::Failed { command: FailedCommand::Other("load review themes"), error: error.to_string() }
            }
        },
        FilesystemCommand::ApplyTheme { settings } => CommandResult::ThemeApplied(
            run_blocking(move || {
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

async fn run_blocking<T>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, JoinError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await
}
