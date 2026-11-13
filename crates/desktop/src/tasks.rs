use clankerdiff_rouge_shell::rouge::reactive::{Scope, Task, spawn, spawn_blocking};
use std::future::Future;

pub(crate) fn spawn_in<T: 'static>(scope: Scope, future: impl Future<Output = T> + 'static) -> Task<T> {
    scope.run(|| spawn(future))
}

pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    spawn_blocking(work).await.ok()
}
