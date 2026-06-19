use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::spawn;
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{Context, LlmError, LlmResponse, LlmResponseStream, StreamingModelProvider};

pub struct FakeLlmProvider {
    responses: Vec<Vec<Result<LlmResponse, LlmError>>>,
    pauses: HashMap<usize, HashMap<usize, Arc<Notify>>>,
    call_count: AtomicUsize,
    /// Captured contexts from each call to `stream_response`
    captured_contexts: Arc<Mutex<Vec<Context>>>,
    display_name: String,
    context_window: Option<u32>,
}

impl FakeLlmProvider {
    pub fn new(responses: Vec<Vec<LlmResponse>>) -> Self {
        let wrapped = responses.into_iter().map(|turn| turn.into_iter().map(Ok).collect()).collect();
        Self::from_results(wrapped)
    }

    pub fn with_single_response(chunks: Vec<LlmResponse>) -> Self {
        Self::new(vec![chunks])
    }

    pub fn from_results(responses: Vec<Vec<Result<LlmResponse, LlmError>>>) -> Self {
        Self {
            responses,
            pauses: HashMap::new(),
            call_count: AtomicUsize::new(0),
            captured_contexts: Arc::new(Mutex::new(Vec::new())),
            display_name: "Fake LLM".to_string(),
            context_window: None,
        }
    }

    pub fn with_display_name(mut self, name: &str) -> Self {
        self.display_name = name.to_string();
        self
    }

    pub fn with_context_window(mut self, window: Option<u32>) -> Self {
        self.context_window = window;
        self
    }

    /// Pause the stream for `turn_index` after emitting the chunk at `chunk_index`
    /// until the returned `Notify` is notified. Enables deterministic mid-stream
    /// tests (e.g. send a user message after the provider has emitted some text).
    pub fn pause_turn_after(mut self, turn_index: usize, chunk_index: usize, notify: Arc<Notify>) -> Self {
        self.pauses.entry(turn_index).or_default().insert(chunk_index, notify);
        self
    }

    /// Returns a handle to the captured contexts that can be used to verify
    /// what contexts were passed to the LLM.
    pub fn captured_contexts(&self) -> Arc<Mutex<Vec<Context>>> {
        Arc::clone(&self.captured_contexts)
    }
}

impl StreamingModelProvider for FakeLlmProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        if let Ok(mut contexts) = self.captured_contexts.lock() {
            contexts.push(context.clone());
        }

        let current_call = self.call_count.fetch_add(1, Ordering::SeqCst);

        let response = if current_call < self.responses.len() {
            self.responses[current_call].clone()
        } else {
            vec![Ok(LlmResponse::done())]
        };

        let pauses = self.pauses.get(&current_call).cloned().unwrap_or_default();
        if pauses.is_empty() {
            return Box::pin(tokio_stream::iter(response));
        }

        let (tx, rx) = mpsc::unbounded_channel();
        spawn(async move {
            for (index, chunk) in response.into_iter().enumerate() {
                if tx.send(chunk).is_err() {
                    return;
                }
                if let Some(notify) = pauses.get(&index) {
                    notify.notified().await;
                }
            }
        });
        Box::pin(UnboundedReceiverStream::new(rx))
    }

    fn display_name(&self) -> String {
        self.display_name.clone()
    }

    fn context_window(&self) -> Option<u32> {
        self.context_window
    }
}
