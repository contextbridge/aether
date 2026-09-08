#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/codex.md"))]

mod continuation;
pub mod oauth;
pub mod provider;
mod session;
mod websocket;

pub const PROVIDER_ID: &str = "codex";

pub use oauth::perform_codex_oauth_flow;
pub use provider::CodexProvider;

#[cfg(test)]
mod test_server;
#[cfg(test)]
mod tests;
