use crate::command::{Command, CommandResult, FailedCommand, FilesystemCommand, GitCommand, TerminalCommand};
use crate::runtime::{agent, files, git};
use acp_utils::client::AcpClientHandle;
use crossterm::{execute, style::Print};
use std::io;

use super::tasks::{ReadTask, TaskSupervisor};

pub struct CommandDispatcher {
    client_handle: AcpClientHandle,
    tasks: TaskSupervisor,
}

impl CommandDispatcher {
    pub fn new(client_handle: impl Into<AcpClientHandle>) -> Self {
        Self { client_handle: client_handle.into(), tasks: TaskSupervisor::default() }
    }

    pub fn dispatch(&mut self, command: Command) -> Option<CommandResult> {
        match command {
            Command::Agent(command) => agent::execute(&self.client_handle, command, &mut self.tasks),
            Command::Filesystem(command) => {
                let key = match &command {
                    FilesystemCommand::IndexFiles { .. } => Some(ReadTask::FileIndex),
                    FilesystemCommand::PrepareSubmission { .. } => Some(ReadTask::AttachmentPreparation),
                    FilesystemCommand::ListThemes => Some(ReadTask::ThemeList),
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
            Command::Git(command) => {
                let read = matches!(command, GitCommand::Load { .. } | GitCommand::LoadFullFile { .. });
                let work = async move { CommandResult::GitDiff(git::execute(command).await) };
                if read {
                    self.tasks.spawn_read(ReadTask::GitReview, work);
                } else {
                    self.tasks.spawn_mutation(work);
                }
                None
            }
            Command::ResolveWorkspace { cwd } => {
                self.tasks.spawn_read(ReadTask::Workspace, async move {
                    let status = git::resolve_workspace_status(&cwd).await;
                    CommandResult::WorkspaceResolved { cwd, status }
                });
                None
            }
            Command::Terminal(command) => execute_terminal(&command),
        }
    }

    pub fn has_pending_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    pub async fn next_result(&mut self) -> Option<CommandResult> {
        self.tasks.next().await
    }

    pub async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }
}

fn execute_terminal(command: &TerminalCommand) -> Option<CommandResult> {
    let TerminalCommand::RingBell = command;
    execute!(io::stdout(), Print("\x07")).err().map(|error| CommandResult::Failed {
        command: FailedCommand::Other("ring the terminal bell"),
        error: error.to_string(),
    })
}
