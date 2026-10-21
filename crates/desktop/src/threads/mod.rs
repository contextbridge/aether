//! Agent threads: each one session with its own agent, driven on the shared Tokio runtime.

mod driver;
mod manager;
mod status;
mod thread;

pub use manager::{Attention, ThreadManager};
pub use status::{ConnectionPhase, ThreadStatus};
pub use thread::{PromptSearchEvent, ReviewEvent, Thread, ThreadEvent, ThreadId};
