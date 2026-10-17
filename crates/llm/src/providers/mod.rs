#![doc = include_str!("../docs/providers.md")]

pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
#[cfg(feature = "codex")]
pub mod codex;
pub mod generic;
pub(crate) mod http;
pub mod local;
pub mod openai;
pub(crate) mod response_stream;
#[cfg(test)]
pub(crate) mod test_capture_server;
