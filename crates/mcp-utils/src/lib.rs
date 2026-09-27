#![doc = include_str!("../README.md")]

mod protocol;
pub mod server;
pub mod testing;
pub mod tool_gateway;

#[cfg(feature = "client")]
pub mod client;

pub use rmcp::ServiceExt;
