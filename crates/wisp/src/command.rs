use crate::settings::UiSettings;
use crate::theme::{Theme, ThemeApplicationError};
use client_core::composer::attachment::PromptAttachment;
use std::path::PathBuf;
use tokio::task::JoinError;

#[derive(Debug, Clone)]
pub enum Command {
    Core(client_core::command::Command),
    Host(HostCommand),
}

#[derive(Debug, Clone)]
pub enum HostCommand {
    ListThemes,
    ListReviewThemes,
    ApplyTheme { settings: Box<UiSettings> },
    IndexFiles { root: PathBuf },
}

#[derive(Debug)]
pub enum HostCommandResult {
    ThemesListed(Vec<String>),
    ReviewThemesListed(Vec<clankerdiff_ratatui::ThemeChoice>),
    ThemeApplied(Result<(Box<UiSettings>, Theme), ThemeApplicationError>),
    FilesIndexed(Vec<PromptAttachment>),
    TaskFailed(JoinError),
}

impl From<client_core::command::Command> for Command {
    fn from(command: client_core::command::Command) -> Self {
        Self::Core(command)
    }
}

impl From<HostCommand> for Command {
    fn from(command: HostCommand) -> Self {
        Self::Host(command)
    }
}

impl From<JoinError> for HostCommandResult {
    fn from(error: JoinError) -> Self {
        Self::TaskFailed(error)
    }
}
