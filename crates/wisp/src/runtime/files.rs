use crate::attachment::{AttachmentOutcome, build_attachments};
use crate::command::{CommandResult, FailedCommand, FilesystemCommand};
use crate::file_index::index_files;
use crate::settings::{list_theme_files, load_theme_file, save_settings};
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
        FilesystemCommand::ApplyTheme { settings, value } => {
            let fallback_settings = settings.clone();
            run_blocking(move || {
                let error = save_settings(&settings).err().map(|error| error.to_string());
                let theme = if value.is_empty() { Theme::default() } else { load_theme_file(&value) };
                CommandResult::ThemeApplied { settings, theme, error }
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
