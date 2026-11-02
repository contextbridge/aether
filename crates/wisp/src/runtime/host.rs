use crate::command::{HostCommand, HostCommandResult};
use crate::platform::{open_url, ring_bell, write_clipboard};
use crate::settings::{UiSettings, list_theme_files, review_theme_choices, save_settings};
use crate::theme::{Theme, ThemeApplicationError};
use std::collections::HashMap;
use tokio::task::{AbortHandle, JoinSet, spawn_blocking};

pub trait Host {
    fn execute(&mut self, command: HostCommand);
}

/// Runs host work off the UI thread. A newer read of the same kind supersedes
/// the one in flight, so a slow read never overwrites a fresher result.
#[derive(Default)]
pub struct TuiHost {
    tasks: JoinSet<(Option<HostRead>, HostCommandResult)>,
    reads: HashMap<HostRead, AbortHandle>,
}

impl Host for TuiHost {
    fn execute(&mut self, command: HostCommand) {
        match command {
            HostCommand::ListThemes => {
                self.read(HostRead::Themes, || HostCommandResult::ThemesListed(list_theme_files()));
            }
            HostCommand::ListReviewThemes => {
                self.read(HostRead::ReviewThemes, || HostCommandResult::ReviewThemesListed(review_theme_choices()));
            }
            HostCommand::ApplyTheme { settings } => {
                self.tasks.spawn(async move { (None, apply_theme(settings).await) });
            }
            // Rung inline: a write from another task could land inside a frame
            // the renderer is still writing.
            HostCommand::RingBell => {
                if let Err(error) = ring_bell() {
                    self.tasks.spawn(async move { (None, HostCommandResult::BellFailed(error)) });
                }
            }
            HostCommand::OpenUrl { elicitation, url } => {
                self.tasks.spawn_blocking(move || {
                    (None, HostCommandResult::UrlOpened { elicitation, result: open_url(&url) })
                });
            }
            HostCommand::CopyUrl { elicitation, url } => {
                self.tasks.spawn_blocking(move || {
                    (None, HostCommandResult::UrlCopied { elicitation, result: write_clipboard(&url) })
                });
            }
        }
    }
}

impl TuiHost {
    pub async fn next(&mut self) -> HostCommandResult {
        loop {
            match self.tasks.join_next_with_id().await {
                Some(Ok((id, (Some(read), result)))) => {
                    if self.reads.get(&read).is_some_and(|handle| handle.id() == id) {
                        self.reads.remove(&read);
                        return result;
                    }
                }
                Some(Ok((_, (None, result)))) => return result,
                Some(Err(error)) if error.is_cancelled() => {}
                Some(Err(error)) => return HostCommandResult::from(error),
                None => return std::future::pending().await,
            }
        }
    }

    pub async fn shutdown(&mut self) {
        for (_, handle) in self.reads.drain() {
            handle.abort();
        }
        while let Some(result) = self.tasks.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                tracing::error!(%error, "background task failed during shutdown");
            }
        }
    }

    fn read(&mut self, key: HostRead, work: impl FnOnce() -> HostCommandResult + Send + 'static) {
        let handle = self.tasks.spawn_blocking(move || (Some(key), work()));
        if let Some(superseded) = self.reads.insert(key, handle) {
            superseded.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum HostRead {
    Themes,
    ReviewThemes,
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
