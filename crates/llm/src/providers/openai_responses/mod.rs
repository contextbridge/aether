//! Shared request mapping and event decoding for the `OpenAI` Responses API.
//!
//! Providers supply their own credentials and HTTP/SSE or WebSocket transport.

pub(crate) mod mappers;
pub(crate) mod streaming;
pub(crate) mod transport;
