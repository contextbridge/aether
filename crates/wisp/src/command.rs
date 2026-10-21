use crate::settings::UiSettings;
use crate::theme::{Theme, ThemeApplicationError};

/// Every command the TUI emits: the client core's plus [`HostCommand`]s.
pub type Command = client_core::command::Command<HostCommand>;
/// Every result the TUI reduces: the client core's plus [`HostCommandResult`]s.
pub type CommandResult = client_core::command::CommandResult<HostCommandResult>;

/// Commands only the terminal UI knows how to run.
#[derive(Debug, Clone)]
pub enum HostCommand {
    ListThemes,
    ListReviewThemes,
    ApplyTheme { settings: Box<UiSettings> },
    RingBell,
}

pub enum HostCommandResult {
    ThemesListed(Vec<String>),
    ReviewThemesListed(Vec<clankerdiff_ratatui::ThemeChoice>),
    ThemeApplied(Result<(Box<UiSettings>, Theme), ThemeApplicationError>),
    TerminalFailed(String),
}
