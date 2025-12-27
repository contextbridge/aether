use futures::future::join_all;
use std::future::Future;
use std::pin::Pin;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Action that middleware handlers can return
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareAction {
    /// Allow the action to proceed
    Allow,
    /// Block the action from executing
    Block,
}

/// Unified event type for agent middleware
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// User sent a message
    UserMessage { content: String },

    /// LLM requested a tool call
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },

    /// Context was compacted to reduce token usage
    ContextCompacted {
        /// Length of the generated summary in characters
        summary_length: usize,
        /// Number of messages that were removed/compacted
        messages_removed: usize,
        /// The compaction strategy that was used (e.g., "tool_result_clearing", "llm_summarization")
        strategy: String,
    },
}

type HandlerFn = Box<dyn Fn(AgentEvent) -> BoxFuture<'static, MiddlewareAction> + Send + Sync>;

/// Middleware for Agents
pub struct Middleware {
    handlers: Vec<HandlerFn>,
}

impl Middleware {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn add_handler<T, U>(&mut self, handler: T)
    where
        T: Fn(AgentEvent) -> U + Send + Sync + 'static,
        U: Future<Output = MiddlewareAction> + Send + 'static,
    {
        self.handlers
            .push(Box::new(move |event| Box::pin(handler(event))));
    }

    /// Trigger all handlers in parallel with the given event.
    /// Returns Block if any handler returns Block, otherwise Allow.
    pub async fn emit(&self, event: AgentEvent) -> MiddlewareAction {
        if self.handlers.is_empty() {
            return MiddlewareAction::Allow;
        }

        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|handler| handler(event.clone()))
            .collect();

        let results = join_all(futures).await;

        // Any Block wins
        if results.contains(&MiddlewareAction::Block) {
            MiddlewareAction::Block
        } else {
            MiddlewareAction::Allow
        }
    }
}

impl Default for Middleware {
    fn default() -> Self {
        Self::new()
    }
}
