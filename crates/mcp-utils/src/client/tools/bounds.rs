use futures::FutureExt;
use futures::future::{Either, select};
use std::future::Future;
use std::pin::pin;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;

pub(super) struct Bounds {
    timeout: Option<Duration>,
    deadline: Option<Instant>,
    cancel: CancellationToken,
}

pub(super) enum Interrupted {
    TimedOut(Duration),
    Cancelled,
}

pub(super) enum Stop<T> {
    Interrupted(Interrupted),
    Failed(T),
}

impl Bounds {
    pub(super) fn new(timeout: Option<Duration>, cancel: CancellationToken) -> Self {
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        Self { timeout, deadline, cancel }
    }

    pub(super) async fn run<T>(&self, future: impl Future<Output = T>) -> Result<T, Interrupted> {
        let bounded = pin!(self.before_deadline(future));
        match select(bounded, pin!(self.cancel.cancelled())).await {
            Either::Left((result, _)) => result,
            Either::Right(((), _)) => Err(Interrupted::Cancelled),
        }
    }

    pub(super) fn try_run<T, E>(
        &self,
        future: impl Future<Output = Result<T, E>>,
    ) -> impl Future<Output = Result<T, Stop<E>>> {
        self.run(future).map(|bounded| match bounded {
            Ok(result) => result.map_err(Stop::Failed),
            Err(interrupted) => Err(Stop::Interrupted(interrupted)),
        })
    }

    async fn before_deadline<T>(&self, future: impl Future<Output = T>) -> Result<T, Interrupted> {
        match (self.deadline, self.timeout) {
            (Some(deadline), Some(timeout)) => {
                timeout_at(deadline, future).await.map_err(|_| Interrupted::TimedOut(timeout))
            }
            _ => Ok(future.await),
        }
    }
}

impl<E> Stop<E> {
    pub(super) fn map_failure<F>(self, f: impl FnOnce(E) -> F) -> Stop<F> {
        match self {
            Self::Interrupted(interrupted) => Stop::Interrupted(interrupted),
            Self::Failed(failure) => Stop::Failed(f(failure)),
        }
    }
}
