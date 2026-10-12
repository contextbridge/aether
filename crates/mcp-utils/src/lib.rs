#![doc = include_str!("../README.md")]

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod gateway;
pub mod server;
#[cfg(all(feature = "client", any(test, feature = "testing")))]
pub mod testing;

pub use rmcp::model;
