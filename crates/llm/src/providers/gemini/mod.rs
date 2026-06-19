#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/google.md"))]

pub mod provider;

pub use provider::{GEMINI_API_BASE, GeminiProvider};
