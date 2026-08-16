#![doc = include_str!("../README.md")]

pub mod display_meta;
mod protocol;
pub mod server;
pub mod status;
pub mod testing;
pub mod transport;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod tool_gateway;

pub use rmcp::ServiceExt;
