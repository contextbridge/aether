#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/openrouter.md"))]

mod provider;
mod types;

pub use provider::*;
pub use types::*;
