use crate::command::{
    Command, CommandResult, FilesystemCommand, GitCommand, GitWatchCommand, TerminalCommand,
};
use crate::git_review::{DiffScope, GitDiffEvent, GitWatchEvent};
use crate::request::RequestId;
use crate::runtime::{agent, files, git};
use acp_utils::client::AcpClientHandle;
use clankerdiff_git::GitRepository;
use clankerdiff_watch::{RepositoryRequest, RepositoryState, RepositoryWatcher, WatchError, WatchOptions};
use crossterm::{execute, style::Print};
use futures::{StreamExt, future::poll_fn};
use std::task::Poll;
use tokio_stream::wrappers::WatchStream;
use std::{io, path::PathBuf, sync::Arc};
use tokio::sync::oneshot;

use super::tasks::{ReadTask, TaskSupervisor};

pub struct CommandDispatcher {
    client_handle: AcpClientHandle,
    tasks: TaskSupervisor,
    git_review: Option<(RequestId, PathBuf)>,
    git_repository: Option<GitRepository>,
    git_watch: Option<RepositoryWatcher>,
    git_updates: Option<WatchStream<RepositoryState>>,
}

impl CommandDispatcher {
    pub fn new(client_handle: AcpClientHandle) -> Self {
        Self {
            client_handle,
            tasks: TaskSupervisor::default(),
            git_review: None,
            git_repository: None,
            git_watch: None,
            git_updates: None,
        }
    }

    pub fn dispatch(&mut self, command: Command) -> Option<CommandResult> {
        match command {
            Command::Agent(command) => agent::execute(&self.client_handle, command, &mut self.tasks),
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
            Command::Git(GitCommand::Apply { review_id, action }) => {
                let Some(repository) = self.git_repository.clone().filter(|_| self.is_review(review_id)) else {
                    return Some(watch_stopped(review_id));
                };
                self.tasks.spawn_git_mutation(repository.root().to_path_buf(), async move {
                    let result = repository.apply(action).await.map_err(Arc::new);
                    CommandResult::GitDiff(GitDiffEvent { review_id, result })
                });
                None
            }
            Command::GitWatch(command) => {
                match command {
                    GitWatchCommand::Open { review_id, working_dir, scope } => {
                        self.close_git_review();
                        self.git_review = Some((review_id, working_dir));
                        self.start_git_watch(scope);
                    }
                    GitWatchCommand::Refresh { review_id, scope } => {
                        if !self.is_review(review_id) {
                            return Some(watch_stopped(review_id));
                        }
                        if let Some(watcher) = &self.git_watch {
                            let requests = watcher.request_tx.clone();
                            self.tasks.spawn_read(ReadTask::GitRefresh, async move {
                                    let (result_tx, completion) = oneshot::channel();
                                    if requests.send(RepositoryRequest::SetScope { scope, result_tx }).await.is_err() {
                                        return watch_stopped(review_id);
                                    }
                                    match completion.await {
                                        Ok(result) => CommandResult::GitDiff(GitDiffEvent { review_id, result }),
                                        Err(_) => watch_stopped(review_id),
                                    }
                            });
                        } else {
                            self.start_git_watch(scope);
                        }
                    }
                    GitWatchCommand::Close { review_id } => {
                        if self.is_review(review_id) {
                            self.close_git_review();
                        }
                    }
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
        !self.tasks.is_empty() || self.git_updates.is_some()
    }

    pub async fn next_result(&mut self) -> Option<CommandResult> {
        loop {
            let result = poll_fn(|cx| {
                if let (Some(updates), Some((review_id, _))) = (&mut self.git_updates, &self.git_review) {
                    let review_id = *review_id;
                    match updates.poll_next_unpin(cx) {
                        Poll::Ready(Some(state)) => return Poll::Ready(Some(CommandResult::GitWatch(
                            GitWatchEvent { review_id, result: Ok(state) },
                        ))),
                        Poll::Ready(None) => {
                            self.close_git_review();
                            return Poll::Ready(Some(watch_stopped(review_id)));
                        }
                        Poll::Pending => {}
                    }
                }
                match self.tasks.poll_result(cx) {
                    Poll::Ready(None) if self.git_updates.is_some() => Poll::Pending,
                    result => result,
                }
            }).await?;
            match result {
                CommandResult::GitWatchStarted { review_id, result } if self.is_review(review_id) => {
                    match result {
                        Ok(started) => {
                            let (repository, watcher) = *started;
                            self.git_updates = Some(WatchStream::new(watcher.state_rx.clone()));
                            self.git_repository = Some(repository);
                            self.git_watch = Some(watcher);
                            self.tasks.spawn_read(ReadTask::GitRefresh, async move {
                                CommandResult::GitDiff(GitDiffEvent { review_id, result: Ok(()) })
                            });
                        }
                        Err(error) => return Some(CommandResult::GitWatch(GitWatchEvent {
                            review_id, result: Err(Arc::new(error)),
                        })),
                    }
                }
                CommandResult::GitWatchStarted { .. } => {}
                result => return Some(result),
            }
        }
    }

    pub async fn shutdown(&mut self) {
        self.close_git_review();
        self.tasks.shutdown().await;
    }

    fn is_review(&self, review_id: RequestId) -> bool {
        self.git_review.as_ref().is_some_and(|(id, _)| *id == review_id)
    }

    fn start_git_watch(&mut self, scope: DiffScope) {
        let Some((review_id, working_dir)) = self.git_review.clone() else { return; };
        self.tasks.cancel_read(ReadTask::GitRefresh);
        self.tasks.spawn_read(ReadTask::GitStart, async move {
            let result = async {
                let repository = GitRepository::discover(working_dir).await?;
                let watcher = RepositoryWatcher::spawn(repository.clone(), scope, WatchOptions::default()).await?;
                Ok(Box::new((repository, watcher)))
            }.await;
            CommandResult::GitWatchStarted { review_id, result }
        });
    }

    fn close_git_review(&mut self) {
        self.tasks.cancel_read(ReadTask::GitStart);
        self.tasks.cancel_read(ReadTask::GitRefresh);
        self.git_updates = None;
        self.git_watch = None;
        self.git_repository = None;
        self.git_review = None;
    }
}

fn watch_stopped(review_id: RequestId) -> CommandResult {
    CommandResult::GitWatch(GitWatchEvent { review_id, result: Err(Arc::new(WatchError::Stopped)) })
}

fn execute_terminal(command: &TerminalCommand) -> Option<CommandResult> {
    let TerminalCommand::RingBell = command;
    execute!(io::stdout(), Print("\x07")).err().map(|error| CommandResult::TerminalFailed(error.to_string()))
}
