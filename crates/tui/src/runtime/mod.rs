use crossterm::event::{Event as CrosstermEvent, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub mod terminal;
pub mod terminal_runtime;
pub use terminal::{MouseCapture, TerminalSession};
pub use terminal_runtime::{TerminalConfig, TerminalRuntime};

pub(crate) struct EventTaskHandle {
    rx: mpsc::UnboundedReceiver<CrosstermEvent>,
    cancel: CancellationToken,
    join: Option<JoinHandle<()>>,
}

impl EventTaskHandle {
    pub(crate) fn rx(&mut self) -> &mut mpsc::UnboundedReceiver<CrosstermEvent> {
        &mut self.rx
    }

    pub(crate) async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for EventTaskHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) fn spawn_terminal_event_task() -> EventTaskHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let mut stream = EventStream::new();
    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = task_cancel.cancelled() => break,
                event = stream.next() => match event {
                    Some(Ok(event)) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Some(Err(err)) => tracing::warn!(%err, "Terminal event error"),
                    None => {
                        tracing::warn!("Terminal event stream ended");
                        break;
                    }
                },
            }
        }
    });
    EventTaskHandle { rx, cancel, join: Some(join) }
}
