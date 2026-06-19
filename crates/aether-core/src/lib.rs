#![doc = include_str!("../README.md")]

pub mod agent_spec;
pub mod context;
pub mod core;
pub mod events;
pub mod mcp;
#[cfg(feature = "testing")]
pub mod testing;

pub use agent_spec::{AgentSpec, AgentSpecExposure};
