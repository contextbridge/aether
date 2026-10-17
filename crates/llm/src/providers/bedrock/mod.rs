#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/amazon-bedrock.md"))]

pub(crate) mod mantle;
pub(crate) mod mappers;
pub(crate) mod provider;
pub(crate) mod streaming;

pub use provider::{AwsCredentials, BedrockProvider};
