#![doc = include_str!("../README.md")]

#[cfg(feature = "analytics")]
pub mod analytics;
pub mod error;
pub mod log;
pub mod model;
pub mod store;
pub mod transcript;

pub use error::{SessionLogError, SessionStoreError};
pub use log::{SessionLine, SessionLog, SessionLogEntry};
pub use model::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent, last_agent_from_events};
pub use store::{
    DiscoveredSessionFile, FileFingerprint, ScanLimits, SessionStore, SessionSummary, discover_session_files,
};
pub use transcript::{context_from_events, conversation_messages_from_events};
