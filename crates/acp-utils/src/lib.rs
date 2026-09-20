#![doc = include_str!("../README.md")]

pub mod config_meta;
pub mod config_option_id;
pub mod content;
pub mod elicitation;
pub mod meta;
pub mod notifications;

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "testing")]
pub mod testing;
