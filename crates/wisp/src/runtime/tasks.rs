use crate::command::{CommandResult, FailedCommand};
use std::collections::HashMap;
use std::future::Future;
use tokio::task::{AbortHandle, JoinError, JoinSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ReadTask {
    AttachmentPreparation,
    FileIndex,
    GitReview,
    ThemeList,
    Workspace,
}

#[derive(Default)]
pub(super) struct TaskSupervisor {
    tasks: JoinSet<TaskCompletion>,
    reads: HashMap<ReadTask, AbortHandle>,
    network: Vec<AbortHandle>,
}

impl TaskSupervisor {
    pub(super) fn spawn_read(
        &mut self,
        key: ReadTask,
        work: impl Future<Output = CommandResult> + Send + 'static,
    ) {
        let handle = self.tasks.spawn(async move { TaskCompletion::Read(key, work.await) });
        if let Some(superseded) = self.reads.insert(key, handle) {
            superseded.abort();
        }
    }

    pub(super) fn spawn_mutation(&mut self, work: impl Future<Output = CommandResult> + Send + 'static) {
        self.tasks.spawn(async move { TaskCompletion::Mutation(work.await) });
    }

    pub(super) fn spawn_network(&mut self, work: impl Future<Output = CommandResult> + Send + 'static) {
        let handle = self.tasks.spawn(async move { TaskCompletion::Network(work.await) });
        self.network.push(handle);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub(super) async fn next(&mut self) -> Option<CommandResult> {
        loop {
            match self.tasks.join_next_with_id().await? {
                Ok((id, TaskCompletion::Read(key, result))) => {
                    let is_current = self.reads.get(&key).is_some_and(|handle| handle.id() == id);
                    if is_current {
                        self.reads.remove(&key);
                        return Some(result);
                    }
                }
                Ok((_, TaskCompletion::Mutation(result) | TaskCompletion::Network(result))) => return Some(result),
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    return Some(CommandResult::Failed {
                        command: FailedCommand::Other("run background task"),
                        error: error.to_string(),
                    });
                }
            }
        }
    }

    pub(super) async fn shutdown(&mut self) {
        for handle in self.reads.values() {
            handle.abort();
        }
        self.reads.clear();
        for handle in self.network.drain(..) {
            handle.abort();
        }
        while let Some(result) = self.tasks.join_next().await {
            log_join_error(result);
        }
    }
}

enum TaskCompletion {
    Read(ReadTask, CommandResult),
    Mutation(CommandResult),
    Network(CommandResult),
}

fn log_join_error(result: Result<TaskCompletion, JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        tracing::error!(%error, "background task failed during runtime shutdown");
    }
}
