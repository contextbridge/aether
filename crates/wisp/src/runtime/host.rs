use super::runner::Host;
use crate::command::{HostCommand, HostCommandResult};
use crate::settings::{UiSettings, list_theme_files, review_theme_choices, save_settings};
use crate::theme::{Theme, ThemeApplicationError};
use client_core::composer::file_index::index_files;
use client_core::runtime::TaskSupervisor;
use tokio::task::spawn_blocking;

#[derive(Default)]
pub struct TuiHost {
    tasks: TaskSupervisor<HostRead, HostCommandResult>,
}

impl Host for TuiHost {
    fn execute(&mut self, command: HostCommand) {
        match command {
            HostCommand::ListThemes => {
                self.tasks
                    .spawn_blocking_read(HostRead::Themes, || HostCommandResult::ThemesListed(list_theme_files()));
            }
            HostCommand::ListReviewThemes => self.tasks.spawn_blocking_read(HostRead::ReviewThemes, || {
                HostCommandResult::ReviewThemesListed(review_theme_choices())
            }),
            HostCommand::IndexFiles { root } => self
                .tasks
                .spawn_blocking_read(HostRead::FileIndex, move || HostCommandResult::FilesIndexed(index_files(&root))),
            HostCommand::ApplyTheme { settings } => self.tasks.spawn_mutation(apply_theme(settings)),
        }
    }
}

impl TuiHost {
    pub async fn next(&mut self) -> HostCommandResult {
        match self.tasks.next().await {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    pub async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum HostRead {
    Themes,
    ReviewThemes,
    FileIndex,
}

async fn apply_theme(settings: Box<UiSettings>) -> HostCommandResult {
    HostCommandResult::ThemeApplied(
        spawn_blocking(move || {
            let theme = Theme::load_selection(&settings.theme)?;
            save_settings(&settings).map_err(ThemeApplicationError::Save)?;
            Ok((settings, theme))
        })
        .await
        .map_err(ThemeApplicationError::Task)
        .and_then(std::convert::identity),
    )
}
