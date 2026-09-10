use crate::attachment::{AttachmentOutcome, build_attachments};
use crate::command::{CommandResult, FailedCommand, FilesystemCommand};
use crate::file_index::index_files;
use crate::settings::{list_theme_files, review_theme_choices, save_settings};
use crate::theme::Theme;
use tokio::task::JoinError;

pub(super) async fn execute(command: FilesystemCommand) -> CommandResult {
    match command {
        FilesystemCommand::IndexFiles { request_id, root } => {
            match run_blocking(move || index_files(&root)).await {
                Ok(files) => CommandResult::FilesIndexed { request_id, files },
                Err(error) => CommandResult::Failed {
                    command: FailedCommand::Other("index files"),
                    error: error.to_string(),
                },
            }
        }
        FilesystemCommand::PrepareSubmission { attachments } => {
            let outcome = run_blocking(move || build_attachments(&attachments)).await.unwrap_or_else(
                |error| AttachmentOutcome {
                    blocks: Vec::new(),
                    placeholders: Vec::new(),
                    warnings: vec![format!("Could not prepare attachments: {error}")],
                },
            );
            CommandResult::SubmissionPrepared(outcome)
        }
        FilesystemCommand::ListThemes => match run_blocking(list_theme_files).await {
            Ok(files) => CommandResult::ThemesListed(files),
            Err(error) => CommandResult::Failed {
                command: FailedCommand::Other("list themes"),
                error: error.to_string(),
            },
        },
        FilesystemCommand::ListReviewThemes => match run_blocking(review_theme_choices).await {
            Ok(choices) => CommandResult::ReviewThemesListed(choices),
            Err(error) => CommandResult::Failed {
                command: FailedCommand::Other("load review themes"),
                error: error.to_string(),
            },
        },
        FilesystemCommand::ApplyTheme { settings, value: _ } => {
            let fallback_settings = settings.clone();
            run_blocking(move || {
                match Theme::load_selection(&settings.theme) {
                    Ok(theme) => {
                        let error = save_settings(&settings).err().map(|error| error.to_string());
                        CommandResult::ThemeApplied { settings, theme, error }
                    }
                    Err(error) => CommandResult::ThemeApplied { settings, theme: Theme::default(), error: Some(error.to_string()) },
                }
            })
            .await
            .unwrap_or_else(|error| CommandResult::ThemeApplied {
                settings: fallback_settings,
                theme: Theme::default(),
                error: Some(format!("Theme task failed: {error}")),
            })
        }
    }
}

async fn run_blocking<T>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, JoinError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await
}
