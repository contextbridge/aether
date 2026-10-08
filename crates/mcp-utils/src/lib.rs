#![doc = include_str!("../README.md")]

mod error;
#[cfg(feature = "client")]
pub mod gateway;
pub mod server;
#[cfg(all(feature = "client", any(test, feature = "testing")))]
pub mod testing;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod config;

pub use error::McpError;
pub use rmcp::model;
