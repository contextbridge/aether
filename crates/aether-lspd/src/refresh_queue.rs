use lsp_types::Uri;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub(crate) struct RefreshQueue {
    state: Arc<Mutex<RefreshQueueState>>,
    wake: Arc<Notify>,
    progress: Arc<Notify>,
}

struct RefreshQueueState {
    pending_queue: VecDeque<Uri>,
    pending_set: HashSet<Uri>,
    scheduled_generation: u64,
    completed_generation: u64,
    bootstrap_in_progress: bool,
    active: bool,
    shutdown: bool,
}

impl RefreshQueue {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RefreshQueueState {
                pending_queue: VecDeque::new(),
                pending_set: HashSet::new(),
                scheduled_generation: 1,
                completed_generation: 0,
                bootstrap_in_progress: true,
                active: false,
                shutdown: false,
            })),
            wake: Arc::new(Notify::new()),
            progress: Arc::new(Notify::new()),
        }
    }

    pub(crate) async fn enqueue(&self, uris: Vec<Uri>) {
        if uris.is_empty() {
            return;
        }

        let mut state = self.state.lock().await;
        let mut added = false;
        for uri in uris {
            if state.pending_set.insert(uri.clone()) {
                state.pending_queue.push_back(uri);
                added = true;
            }
        }

        if !added {
            return;
        }

        if !state.bootstrap_in_progress {
            state.scheduled_generation += 1;
        }
        drop(state);
        self.wake.notify_one();
    }

    pub(crate) async fn complete_bootstrap(&self) {
        let mut should_notify = false;
        {
            let mut state = self.state.lock().await;
            state.bootstrap_in_progress = false;
            if !state.active && state.pending_queue.is_empty() {
                state.completed_generation = state.scheduled_generation;
                should_notify = true;
            }
        }

        if should_notify {
            self.progress.notify_waiters();
        }
        self.wake.notify_one();
    }

    pub(crate) async fn wait_for_current_generation(&self, timeout: Duration) {
        let target = self.state.lock().await.scheduled_generation;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            {
                let state = self.state.lock().await;
                if state.completed_generation >= target {
                    return;
                }
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }

            tokio::select! {
                () = self.progress.notified() => {}
                () = tokio::time::sleep(remaining) => return,
            }
        }
    }

    pub(crate) async fn recv(&self) -> Option<Uri> {
        loop {
            let wake = self.wake.notified();
            let mut should_notify = false;
            {
                let mut state = self.state.lock().await;
                if state.shutdown {
                    return None;
                }

                if let Some(uri) = state.pending_queue.pop_front() {
                    state.pending_set.remove(&uri);
                    state.active = true;
                    return Some(uri);
                }

                state.active = false;
                if !state.bootstrap_in_progress && state.completed_generation != state.scheduled_generation {
                    state.completed_generation = state.scheduled_generation;
                    should_notify = true;
                }
            }

            if should_notify {
                self.progress.notify_waiters();
            }

            wake.await;
        }
    }

    pub(crate) async fn shutdown(&self) {
        {
            let mut state = self.state.lock().await;
            state.shutdown = true;
        }
        self.wake.notify_waiters();
        self.progress.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn enqueue_deduplicates_uris() {
        let queue = RefreshQueue::new();
        queue.complete_bootstrap().await;

        let a = uri("file:///a.rs");
        queue.enqueue(vec![a.clone(), a.clone()]).await;

        assert_eq!(queue.recv().await, Some(a));
    }

    #[tokio::test]
    async fn wait_for_current_generation_resolves_when_drained() {
        let queue = RefreshQueue::new();
        queue.complete_bootstrap().await;

        let a = uri("file:///a.rs");
        let b = uri("file:///b.rs");
        queue.enqueue(vec![a, b]).await;

        let wait_queue = queue.clone();
        let waiter = tokio::spawn(async move {
            wait_queue.wait_for_current_generation(Duration::from_secs(2)).await;
        });

        assert!(queue.recv().await.is_some());
        assert!(queue.recv().await.is_some());

        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn bootstrap_completion_without_pending_wakes_waiters() {
        let queue = RefreshQueue::new();

        let wait_queue = queue.clone();
        let waiter = tokio::spawn(async move {
            wait_queue.wait_for_current_generation(Duration::from_secs(2)).await;
        });

        tokio::task::yield_now().await;
        queue.complete_bootstrap().await;

        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_causes_recv_to_return_none() {
        let queue = RefreshQueue::new();
        queue.complete_bootstrap().await;

        queue.enqueue(vec![uri("file:///a.rs")]).await;
        queue.shutdown().await;

        assert_eq!(queue.recv().await, None);
    }

    #[tokio::test]
    async fn shutdown_wakes_waiting_receiver() {
        let queue = RefreshQueue::new();
        queue.complete_bootstrap().await;

        let wait_queue = queue.clone();
        let receiver = tokio::spawn(async move { wait_queue.recv().await });

        tokio::task::yield_now().await;
        queue.shutdown().await;

        assert_eq!(receiver.await.unwrap(), None);
    }
}
