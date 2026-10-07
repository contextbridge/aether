#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/openai.md"))]

pub mod mappers;
pub mod provider;

pub use provider::*;
