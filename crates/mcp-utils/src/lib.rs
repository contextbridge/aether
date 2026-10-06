#![doc = include_str!("../README.md")]

mod protocol;
pub mod server;
pub mod testing;

#[cfg(feature = "client")]
pub mod aggregate;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "ipc")]
pub mod tool_gateway;

pub use rmcp;
pub use rmcp::ServiceExt;
