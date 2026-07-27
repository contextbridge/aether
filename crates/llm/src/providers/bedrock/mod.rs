#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/amazon-bedrock.md"))]

pub mod mantle;
pub mod mappers;
pub mod provider;
pub mod streaming;

pub use provider::{AwsCredentials, BedrockProvider};
