use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::task::{Context, Poll};
use tokio::task::{AbortHandle, JoinError, JoinSet};

pub struct TaskSupervisor<T, U> {
    tasks: JoinSet<TaskCompletion<T, U>>,
    reads: HashMap<T, AbortHandle>,
    network: Vec<AbortHandle>,
}

impl<T: Copy + Eq + Hash + Send + 'static, U: From<JoinError> + Send + 'static> TaskSupervisor<T, U> {
    pub fn spawn_blocking_read(&mut self, key: T, work: impl FnOnce() -> U + Send + 'static) {
        let handle = self.tasks.spawn_blocking(move || TaskCompletion::Read(key, work()));
        if let Some(superseded) = self.reads.insert(key, handle) {
            superseded.abort();
        }
    }

    pub fn spawn_mutation(&mut self, work: impl Future<Output = U> + Send + 'static) {
        self.tasks.spawn(async move { TaskCompletion::Mutation(work.await) });
    }

    pub fn spawn_network(&mut self, work: impl Future<Output = U> + Send + 'static) {
        self.network.retain(|handle| !handle.is_finished());
        self.network.push(self.tasks.spawn(async move { TaskCompletion::Mutation(work.await) }));
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub async fn next(&mut self) -> Option<U> {
        std::future::poll_fn(|cx| self.poll_next(cx)).await
    }

    pub async fn shutdown(&mut self) {
        for handle in self.reads.values().chain(self.network.iter()) {
            handle.abort();
        }
        self.reads.clear();
        self.network.clear();
        while let Some(result) = self.tasks.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                tracing::error!(%error, "background task failed during shutdown");
            }
        }
    }

    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<U>> {
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
                Some(Err(error)) => return Poll::Ready(Some(U::from(error))),
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<T, U> Default for TaskSupervisor<T, U> {
    fn default() -> Self {
        Self { tasks: JoinSet::new(), reads: HashMap::new(), network: Vec::new() }
    }
}

enum TaskCompletion<T, U> {
    Read(T, U),
    Mutation(U),
}
