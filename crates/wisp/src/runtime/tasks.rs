use crate::command::CommandResult;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::task::{Context, Poll};
use tokio::sync::oneshot::{self, error::TryRecvError};
use tokio::task::{AbortHandle, JoinError, JoinSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ReadTask {
    AttachmentPreparation,
    FileIndex,
    ThemeList,
    ReviewThemeList,
    Workspace,
    GitStart,
    GitRefresh,
}

#[derive(Default)]
pub(super) struct TaskSupervisor {
    tasks: JoinSet<TaskCompletion>,
    reads: HashMap<ReadTask, AbortHandle>,
    network: Vec<AbortHandle>,
    git_mutations: HashMap<PathBuf, oneshot::Receiver<()>>,
}

impl TaskSupervisor {
    pub(super) fn spawn_read(&mut self, key: ReadTask, work: impl Future<Output = CommandResult> + Send + 'static) {
        let handle = self.tasks.spawn(async move { TaskCompletion::Read(key, work.await) });
        if let Some(superseded) = self.reads.insert(key, handle) {
            superseded.abort();
        }
    }

    pub(super) fn cancel_read(&mut self, key: ReadTask) {
        if let Some(handle) = self.reads.remove(&key) {
            handle.abort();
        }
    }

    pub(super) fn spawn_mutation(&mut self, work: impl Future<Output = CommandResult> + Send + 'static) {
        self.tasks.spawn(async move { TaskCompletion::Mutation(work.await) });
    }

    pub(super) fn spawn_git_mutation(
        &mut self,
        repo_root: PathBuf,
        task: impl Future<Output = CommandResult> + Send + 'static,
    ) {
        self.git_mutations.retain(|_, completion| matches!(completion.try_recv(), Err(TryRecvError::Empty)));
        let (finished, completion) = oneshot::channel();
        let previous = self.git_mutations.insert(repo_root, completion);
        self.spawn_mutation(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let result = task.await;
            let _ = finished.send(());
            result
        });
    }

    pub(super) fn submit_network(&mut self, work: impl Future<Output = CommandResult> + Send + 'static) {
        self.network.retain(|handle| !handle.is_finished());
        self.network.push(self.tasks.spawn(async move { TaskCompletion::Mutation(work.await) }));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub(super) fn poll_result(&mut self, cx: &mut Context<'_>) -> Poll<Option<CommandResult>> {
        loop {
            match std::task::ready!(self.tasks.poll_join_next_with_id(cx)) {
                Some(Ok((id, TaskCompletion::Read(key, result)))) => {
                    if self.reads.get(&key).is_some_and(|handle| handle.id() == id) {
                        self.reads.remove(&key);
                        return Poll::Ready(Some(result));
                    }
                }
                Some(Ok((_, TaskCompletion::Mutation(result)))) => return Poll::Ready(Some(result)),
                Some(Err(error)) if error.is_cancelled() => {}
                Some(Err(error)) => return Poll::Ready(Some(CommandResult::BackgroundFailed(error.to_string()))),
                None => return Poll::Ready(None),
            }
        }
    }

    pub(super) async fn shutdown(&mut self) {
        for handle in self.reads.values().chain(self.network.iter()) {
            handle.abort();
        }
        self.reads.clear();
        self.network.clear();
        while let Some(result) = self.tasks.join_next().await {
            log_join_error(result);
        }
        self.git_mutations.clear();
    }
}

enum TaskCompletion {
    Read(ReadTask, CommandResult),
    Mutation(CommandResult),
}

fn log_join_error(result: Result<TaskCompletion, JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        tracing::error!(%error, "background task failed during runtime shutdown");
    }
}
