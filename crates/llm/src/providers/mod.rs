#![doc = include_str!("../docs/providers.md")]

pub(crate) mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
#[cfg(feature = "codex")]
pub mod codex;
#[cfg(test)]
mod fixture_tests;
pub mod generic;
pub(crate) mod http;
pub mod local;
pub(crate) mod openai;
pub(crate) mod response_stream;
#[cfg(test)]
pub(crate) mod test_capture_server;
