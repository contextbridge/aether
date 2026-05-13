#![doc = include_str!("../README.md")]

pub mod alloyed;
pub mod catalog;
mod chat_message;
mod context;
mod credential;
pub mod error;
mod llm_response;
pub mod parser;
pub mod provider;
pub mod providers;
mod reasoning;
pub mod testing;
mod tool_schema;
mod tools;
pub mod types;

pub use catalog::LlmModel;
pub use chat_message::{AssistantReasoning, ChatMessage, ContentBlock, EncryptedReasoningContent};
pub use context::Context;
pub use credential::ProviderCredential;
pub use error::{ContextOverflowError, LlmError, Result};
pub use llm_response::{LlmResponse, StopReason, TokenUsage};
pub use provider::{LlmResponseStream, ProviderFactory, StreamingModelProvider};
pub use reasoning::ReasoningEffort;
pub use tools::*;

#[cfg(feature = "codex")]
pub use providers::codex::{CodexProvider, perform_codex_oauth_flow};
