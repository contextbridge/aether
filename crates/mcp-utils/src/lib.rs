#![doc = include_str!("../README.md")]

pub mod display_meta;
mod protocol;
pub mod request_context;
pub mod server;
pub mod status;
pub mod testing;
pub mod tool_exposure;
pub mod tool_gateway;
pub mod tool_policy;
pub mod transport;

#[cfg(feature = "client")]
pub mod client;

pub use rmcp::ServiceExt;
