use crate::command::{CommandResult, FilesystemCommand};
use crate::composer::attachment::{AttachmentOutcome, build_attachments};
use crate::composer::file_index::index_files;
use tokio::task::spawn_blocking;

/// Runs `work` on the blocking pool and maps its value through `done`; a
/// panicked task becomes [`CommandResult::BackgroundFailed`] prefixed with `label`.
pub async fn run_blocking<T: Send + 'static, U>(
    label: &str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> CommandResult<U>,
) -> CommandResult<U> {
    match spawn_blocking(work).await {
        Ok(value) => done(value),
        Err(error) => CommandResult::BackgroundFailed(format!("{label}: {error}")),
    }
}

pub(super) async fn execute<T>(command: FilesystemCommand) -> CommandResult<T> {
    match command {
        FilesystemCommand::IndexFiles { request_id, root } => {
            let indexed = |files| CommandResult::FilesIndexed { request_id, files };
            run_blocking("Failed to index files", move || index_files(&root), indexed).await
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
    }
}
