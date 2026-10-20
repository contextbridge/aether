#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/amazon-bedrock.md"))]

mod converse;
mod invoke;
pub(crate) mod mantle;
pub(crate) mod provider;
mod sdk;

pub use provider::{AwsCredentials, BedrockProvider};
