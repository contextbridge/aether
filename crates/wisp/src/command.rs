use crate::settings::UiSettings;
use crate::theme::{Theme, ThemeApplicationError};
use client_core::composer::attachment::PromptAttachment;
use client_core::elicitation::ElicitationId;
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
    ApplyTheme {
        settings: Box<UiSettings>,
    },
    IndexFiles {
        root: PathBuf,
    },
    RingBell,
    /// Opens a URL request's link in the browser; the result goes back to the
    /// request that asked.
    OpenUrl {
        elicitation: ElicitationId,
        url: String,
    },
    CopyUrl {
        elicitation: ElicitationId,
        url: String,
    },
}

#[derive(Debug)]
pub enum HostCommandResult {
    ThemesListed(Vec<String>),
    ReviewThemesListed(Vec<clankerdiff_ratatui::ThemeChoice>),
    ThemeApplied(Result<(Box<UiSettings>, Theme), ThemeApplicationError>),
    FilesIndexed(Vec<PromptAttachment>),
    BellFailed(String),
    UrlOpened { elicitation: ElicitationId, result: Result<(), String> },
    UrlCopied { elicitation: ElicitationId, result: Result<(), String> },
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
