use crate::command::{CommandResult, HostCommand, HostCommandResult};
use crate::settings::{UiSettings, list_theme_files, review_theme_choices, save_settings};
use crate::theme::{Theme, ThemeApplicationError};
use client_core::runtime::{HostExecutor, ReadTask, TaskSupervisor, run_blocking};
use crossterm::{execute, style::Print};
use std::io;
use tokio::task::spawn_blocking;

/// Runs the TUI's [`HostCommand`]s: theme files and the terminal bell.
pub struct TuiHost;

impl HostExecutor for TuiHost {
    type Command = HostCommand;
    type Result = HostCommandResult;

    fn execute(
        &mut self,
        command: HostCommand,
        tasks: &mut TaskSupervisor<HostCommandResult>,
    ) -> Option<CommandResult> {
        match command {
            HostCommand::ListThemes => tasks.spawn_read(ReadTask::Host(THEME_LIST), list_themes()),
            HostCommand::ListReviewThemes => tasks.spawn_read(ReadTask::Host(REVIEW_THEME_LIST), list_review_themes()),
            HostCommand::ApplyTheme { settings } => tasks.spawn_mutation(apply_theme(settings)),
            HostCommand::RingBell => return ring_bell(),
        }
        None
    }
}

const THEME_LIST: &str = "theme-list";
const REVIEW_THEME_LIST: &str = "review-theme-list";

async fn list_themes() -> CommandResult {
    run_blocking("Failed to list themes", list_theme_files, |files| {
        CommandResult::Host(HostCommandResult::ThemesListed(files))
    })
    .await
}

async fn list_review_themes() -> CommandResult {
    run_blocking("Failed to load review themes", review_theme_choices, |choices| {
        CommandResult::Host(HostCommandResult::ReviewThemesListed(choices))
    })
    .await
}

async fn apply_theme(settings: Box<UiSettings>) -> CommandResult {
    CommandResult::Host(HostCommandResult::ThemeApplied(
        spawn_blocking(move || {
            let theme = Theme::load_selection(&settings.theme)?;
            save_settings(&settings).map_err(ThemeApplicationError::Save)?;
            Ok((settings, theme))
        })
        .await
        .map_err(ThemeApplicationError::Task)
        .and_then(std::convert::identity),
    ))
}

fn ring_bell() -> Option<CommandResult> {
    execute!(io::stdout(), Print("\x07"))
        .err()
        .map(|error| CommandResult::Host(HostCommandResult::TerminalFailed(error.to_string())))
}
