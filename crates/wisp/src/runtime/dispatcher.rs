use crate::command::{
    Command, CommandResult, FailedCommand, FilesystemCommand, GitCommand, GitWatchCommand, TerminalCommand,
};
use crate::git_review::{DiffScope, GitDiffEvent, GitWatchEvent};
use crate::request::RequestId;
use crate::runtime::{agent, files, git};
use acp_utils::client::AcpClientHandle;
use clankerdiff_git::GitRepository;
use clankerdiff_watch::{RepositoryRequest, RepositoryWatcher, WatchError, WatchOptions};
use crossterm::{execute, style::Print};
use futures::{FutureExt, future::BoxFuture};
use std::{io, path::PathBuf, sync::Arc};
use tokio::sync::oneshot;

use super::tasks::{ReadTask, TaskSupervisor};

pub struct CommandDispatcher {
    client_handle: AcpClientHandle,
    tasks: TaskSupervisor,
    git_review: Option<(RequestId, PathBuf)>,
    git_repository: Option<GitRepository>,
    git_watch: Option<RepositoryWatcher>,
    git_start: Option<BoxFuture<'static, Result<(GitRepository, RepositoryWatcher), WatchError>>>,
    git_refresh: Option<BoxFuture<'static, CommandResult>>,
}

impl CommandDispatcher {
    pub fn new(client_handle: AcpClientHandle) -> Self {
        Self {
            client_handle,
            tasks: TaskSupervisor::default(),
            git_review: None,
            git_repository: None,
            git_watch: None,
            git_start: None,
            git_refresh: None,
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
                            self.git_refresh = Some(
                                async move {
                                    let (result_tx, completion) = oneshot::channel();
                                    if requests.send(RepositoryRequest::SetScope { scope, result_tx }).await.is_err() {
                                        return watch_stopped(review_id);
                                    }
                                    match completion.await {
                                        Ok(result) => CommandResult::GitDiff(GitDiffEvent { review_id, result }),
                                        Err(_) => watch_stopped(review_id),
                                    }
                                }
                                .boxed(),
                            );
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
        !self.tasks.is_empty() || self.git_start.is_some() || self.git_watch.is_some() || self.git_refresh.is_some()
    }

    pub async fn next_result(&mut self) -> Option<CommandResult> {
        loop {
            tokio::select! {
                biased;
                started = async { self.git_start.as_mut().expect("starting watcher").await }, if self.git_start.is_some() => {
                    self.git_start = None;
                    let review_id = self.git_review.as_ref().expect("active review").0;
                    let result = started.map(|(repository, mut watcher)| {
                        let state = watcher.state_rx.borrow_and_update().clone();
                        self.git_repository = Some(repository);
                        self.git_watch = Some(watcher);
                        self.git_refresh = Some(async move {
                            CommandResult::GitDiff(GitDiffEvent { review_id, result: Ok(()) })
                        }.boxed());
                        state
                    }).map_err(Arc::new);
                    return Some(CommandResult::GitWatch(GitWatchEvent { review_id, result }));
                }
                changed = async { self.git_watch.as_mut().expect("active watcher").state_rx.changed().await }, if self.git_watch.is_some() => {
                    let review_id = self.git_review.as_ref().expect("active review").0;
                    if changed.is_err() {
                        self.git_watch = None;
                        self.git_repository = None;
                        self.git_refresh = None;
                        return Some(watch_stopped(review_id));
                    }
                    let state = self.git_watch.as_mut().expect("active watcher").state_rx.borrow_and_update().clone();
                    return Some(CommandResult::GitWatch(GitWatchEvent { review_id, result: Ok(state) }));
                }
                result = async { self.git_refresh.as_mut().expect("pending refresh").await }, if self.git_refresh.is_some() => {
                    self.git_refresh = None;
                    return Some(result);
                }
                result = self.tasks.next(), if !self.tasks.is_empty() => {
                    if result.is_some() {
                        return result;
                    }
                }
                else => return None,
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
        let working_dir = self.git_review.as_ref().expect("active review").1.clone();
        self.git_refresh = None;
        self.git_start = Some(
            async move {
                let repository = GitRepository::discover(working_dir).await?;
                let watcher = RepositoryWatcher::spawn(repository.clone(), scope, WatchOptions::default()).await?;
                Ok((repository, watcher))
            }
            .boxed(),
        );
    }

    fn close_git_review(&mut self) {
        self.git_start = None;
        self.git_refresh = None;
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
    execute!(io::stdout(), Print("\x07")).err().map(|error| CommandResult::Failed {
        command: FailedCommand::Other("ring the terminal bell"),
        error: error.to_string(),
    })
}
