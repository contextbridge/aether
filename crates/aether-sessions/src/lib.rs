#![doc = include_str!("../README.md")]

pub mod error;
pub mod log;
pub mod model;
pub mod transcript;

pub use error::SessionLogError;
pub use log::{SessionLine, SessionLog, SessionLogEntry};
pub use model::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent, last_agent_from_events};
pub use transcript::{context_from_events, conversation_messages_from_events};
