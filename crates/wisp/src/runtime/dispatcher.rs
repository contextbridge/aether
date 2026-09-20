use crate::command::{Command, CommandResult, FilesystemCommand, TerminalCommand};
use crate::runtime::{agent, files};
use acp_utils::client::AcpClientHandle;
use crossterm::{execute, style::Print};
use std::io;

use super::git_review::GitReviewRuntime;
use super::tasks::{ReadTask, TaskSupervisor};

pub struct CommandDispatcher {
    client_handle: AcpClientHandle,
    tasks: TaskSupervisor,
    git_review: GitReviewRuntime,
}

impl CommandDispatcher {
    pub fn new(client_handle: AcpClientHandle) -> Self {
        Self {
            git_review: GitReviewRuntime::new(client_handle.clone()),
            client_handle,
            tasks: TaskSupervisor::default(),
        }
    }

    pub fn dispatch(&mut self, command: Command) -> Option<CommandResult> {
        match command {
            Command::Agent(command) => agent::execute(&self.client_handle, command, &mut self.tasks),
            Command::GitReview(command) => self.git_review.dispatch(command, &mut self.tasks),
            Command::Filesystem(command) => {
                let key = match &command {
                    FilesystemCommand::IndexFiles { .. } => Some(ReadTask::FileIndex),
                    FilesystemCommand::PrepareSubmission { .. } => Some(ReadTask::AttachmentPreparation),
                    FilesystemCommand::ListThemes => Some(ReadTask::ThemeList),
                    FilesystemCommand::ListReviewThemes => Some(ReadTask::ReviewThemeList),
                    FilesystemCommand::ApplyTheme { .. } => None,
                };
                let work = async move { files::execute(command).await };
                if let Some(key) = key {
                    self.tasks.spawn_read(key, work);
                } else {
                    self.tasks.spawn_mutation(work);
                }
                None
            }
            Command::Terminal(command) => execute_terminal(&command),
        }
    }

    pub fn has_pending_tasks(&self) -> bool {
        !self.tasks.is_empty() || self.git_review.is_active()
    }

    pub async fn next_result(&mut self) -> Option<CommandResult> {
        loop {
            if !self.git_review.is_active() && self.tasks.is_empty() {
                return None;
            }
            tokio::select! {
                state = self.git_review.changed() => {
                    match state {
                        Some(state) => return Some(CommandResult::GitReview(state)),
                        None => self.git_review.close(),
                    }
                }
                result = std::future::poll_fn(|cx| self.tasks.poll_result(cx)), if !self.tasks.is_empty() => {
                    if let Some(result) = result {
                        return Some(result);
                    }
                }
            }
        }
    }

    pub async fn shutdown(&mut self) {
        self.git_review.close();
        self.client_handle.disconnect().await;
        self.tasks.shutdown().await;
    }
}

fn execute_terminal(command: &TerminalCommand) -> Option<CommandResult> {
    let TerminalCommand::RingBell = command;
    execute!(io::stdout(), Print("\x07")).err().map(|error| CommandResult::TerminalFailed(error.to_string()))
}
