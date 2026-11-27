#![doc = include_str!("../README.md")]

#[cfg(feature = "client")]
pub mod client;
mod displayed;
#[cfg(feature = "client")]
pub mod gateway;
pub mod server;
#[cfg(all(feature = "client", any(test, feature = "testing")))]
pub mod testing;
pub mod tool_kind;

pub use displayed::Displayed;
pub use rmcp::model;
