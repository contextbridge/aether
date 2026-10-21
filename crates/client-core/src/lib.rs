//! UI-agnostic client core for Aether's ACP frontends.
//!
//! A host (a terminal UI, a desktop app) connects with [`session::Session`]
//! and drives one [`session_core::SessionCore`] per session: agent events,
//! command results, and user intents go in, and [`session_core::Effect`]s come
//! out. The commands those effects dispatch run through
//! [`runtime::CommandDispatcher`]. Host-specific commands ride along in
//! [`command::Command::Host`] and are executed by the host's own
//! [`runtime::HostExecutor`].

pub mod command;
pub mod command_entry;
pub mod composer;
pub mod elicitation;
pub mod error;
pub mod plan_tracker;
pub mod request;
pub mod runtime;
pub mod session;
pub mod session_config_view;
pub mod session_core;
pub mod session_model;
#[cfg(feature = "testing")]
pub mod testing;
pub mod workspace_status;
