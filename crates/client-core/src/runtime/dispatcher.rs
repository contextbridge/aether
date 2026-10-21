use super::git_review::GitReviewRuntime;
use super::tasks::{ReadTask, TaskSupervisor};
use super::{agent, files};
use crate::command::{Command, CommandResult, FilesystemCommand};
use acp_utils::client::AcpClientHandle;
use std::convert::Infallible;

/// Runs the commands only a host knows about, such as UI-specific
/// persistence or terminal effects.
pub trait HostExecutor {
    type Command;
    type Result: Send + 'static;

    /// Executes `command`, either answering at once or spawning its work on `tasks`.
    fn execute(
        &mut self,
        command: Self::Command,
        tasks: &mut TaskSupervisor<Self::Result>,
    ) -> Option<CommandResult<Self::Result>>;
}

/// The executor for a host that has no commands of its own.
pub struct NoHost;

/// Executes [`Command`]s against an agent connection, the local filesystem,
/// the git review client, and the host's [`HostExecutor`].
pub struct CommandDispatcher<T: HostExecutor> {
    client_handle: AcpClientHandle,
    tasks: TaskSupervisor<T::Result>,
    git_review: GitReviewRuntime,
    host: T,
}

impl<T: HostExecutor> CommandDispatcher<T> {
    pub fn new(client_handle: AcpClientHandle, host: T) -> Self {
        Self {
            git_review: GitReviewRuntime::new(client_handle.clone()),
            client_handle,
            tasks: TaskSupervisor::default(),
            host,
        }
    }

    pub fn dispatch(&mut self, command: Command<T::Command>) -> Option<CommandResult<T::Result>> {
        match command {
            Command::Agent(command) => agent::execute(&self.client_handle, command, &mut self.tasks),
            Command::GitReview(command) => self.git_review.dispatch(command, &mut self.tasks),
            Command::Filesystem(command) => {
                let key = match &command {
                    FilesystemCommand::IndexFiles { .. } => ReadTask::FileIndex,
                    FilesystemCommand::PrepareSubmission { .. } => ReadTask::AttachmentPreparation,
                };
                self.tasks.spawn_read(key, files::execute(command));
                None
            }
            Command::Host(command) => self.host.execute(command, &mut self.tasks),
        }
    }

    pub fn has_pending_tasks(&self) -> bool {
        !self.tasks.is_empty() || self.git_review.is_active()
    }

    pub async fn next_result(&mut self) -> Option<CommandResult<T::Result>> {
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

impl HostExecutor for NoHost {
    type Command = Infallible;
    type Result = Infallible;

    fn execute(
        &mut self,
        command: Infallible,
        _: &mut TaskSupervisor<Infallible>,
    ) -> Option<CommandResult<Infallible>> {
        match command {}
    }
}
