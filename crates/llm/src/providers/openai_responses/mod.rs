//! Shared building blocks for providers that speak the `OpenAI` Responses API.
//!
//! Several endpoints serve the same `POST {base}/responses` wire contract behind
//! different hosts and authentication schemes -- the `ChatGPT` Codex backend and
//! the Bedrock Mantle endpoint among them. Request mapping and SSE event
//! processing are identical across all of them, so they live here and each
//! provider supplies only its own transport and credentials.

pub(crate) mod mappers;
pub(crate) mod streaming;
pub(crate) mod transport;
