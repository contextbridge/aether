#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/codex.md"))]

pub mod mappers;
pub mod oauth;
pub mod provider;
pub mod streaming;

pub const PROVIDER_ID: &str = "codex";

pub use oauth::perform_codex_oauth_flow;
pub use provider::CodexProvider;
