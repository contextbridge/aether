#![doc = include_str!("../docs/providers.md")]

pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
#[cfg(feature = "codex")]
pub mod codex;
pub mod gemini;
pub mod local;
pub mod openai;
pub mod openai_compatible;
pub(crate) mod openai_responses;
pub mod openrouter;
#[cfg(test)]
pub(crate) mod test_capture_server;
pub(crate) mod tool_call_collector;
