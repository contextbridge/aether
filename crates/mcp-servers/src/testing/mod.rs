//! Shared fakes and builders for exercising `aether-mcp-servers` without real I/O.
//!
//! Compiled for the crate's own tests and for any consumer that enables the
//! `test-helpers` feature. Prefer these over hand-rolling per-suite fakes.

#[cfg(feature = "coding")]
mod web_fetch;

#[cfg(feature = "coding")]
pub use web_fetch::FakeHttpClient;
